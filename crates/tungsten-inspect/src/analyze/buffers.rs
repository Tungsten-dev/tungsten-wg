//! Buffer lifetimes and simulated reuse, per entry and per time axis (SPEC.md §6.4).
//!
//! Lifetimes and slot assignment are separate steps, so a real assignment from the interface
//! (§6.5) can replace or sit next to `assign` without touching `lifetimes`.

use std::collections::HashMap;

use tungsten_interface::Instance;

use super::Ctx;
use super::schedule::EntrySchedule;
use crate::model::{AssignmentSource, Axis, BufferAxis, BufferInfo, EntryBuffers, InstanceInfo};

/// One buffer of the entry: one per instance in its closure.
struct Buffer<'a> {
    inst: Instance,
    info: &'a InstanceInfo,
    /// The entry's own output: it must outlive the run.
    pinned: bool,
}

/// Inclusive interval of time units; `end == None` is ∞ (pinned).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Lifetime {
    start: usize,
    end: Option<usize>,
}

impl Lifetime {
    fn overlaps(&self, other: &Lifetime) -> bool {
        let le = |a: usize, end: Option<usize>| end.is_none_or(|e| a <= e);
        le(self.start, other.end) && le(other.start, self.end)
    }

    fn contains(&self, unit: usize) -> bool {
        self.start <= unit && self.end.is_none_or(|e| unit <= e)
    }
}

pub(crate) fn entry_buffers(
    ctx: &Ctx,
    entry: &Instance,
    schedule: &EntrySchedule,
    by_id: &HashMap<&str, &InstanceInfo>,
) -> EntryBuffers {
    let buffers: Vec<Buffer> = schedule
        .steps
        .iter()
        .map(|inst| Buffer {
            inst: *inst,
            info: by_id[ctx.instance_id(inst).as_str()],
            pinned: inst == entry,
        })
        .collect();

    let cpu_units: Vec<Vec<Instance>> = schedule.steps.iter().map(|i| vec![*i]).collect();
    EntryBuffers {
        cpu: axis(Axis::Cpu, &buffers, &cpu_units),
        gpu: axis(Axis::Gpu, &buffers, &schedule.waves),
    }
}

/// `units[u]` are the instances written in time unit `u`.
fn axis(axis: Axis, buffers: &[Buffer], units: &[Vec<Instance>]) -> BufferAxis {
    let index: HashMap<Instance, usize> = buffers
        .iter()
        .enumerate()
        .map(|(i, b)| (b.inst, i))
        .collect();
    let by_id: HashMap<&str, usize> = buffers
        .iter()
        .enumerate()
        .map(|(i, b)| (b.info.id.as_str(), i))
        .collect();
    let mut unit_of = vec![0; buffers.len()];
    for (u, insts) in units.iter().enumerate() {
        for inst in insts {
            unit_of[index[inst]] = u;
        }
    }

    // producers[b]: distinct buffers that b reads.
    let producers: Vec<Vec<usize>> = buffers
        .iter()
        .map(|b| {
            let mut ps: Vec<usize> = b
                .info
                .inputs
                .iter()
                .map(|e| {
                    *by_id
                        .get(e.producer.as_str())
                        .expect("producer outside the entry's closure")
                })
                .collect();
            ps.sort();
            ps.dedup();
            ps
        })
        .collect();

    let lifetimes = lifetimes(buffers, &unit_of, &producers);
    let (slot_of, aliases, slots) = assign(buffers, &lifetimes, &producers);

    let live_bytes: Vec<usize> = (0..units.len())
        .map(|u| {
            buffers
                .iter()
                .zip(&lifetimes)
                .filter(|(_, l)| l.contains(u))
                .map(|(b, _)| b.info.bytes)
                .sum()
        })
        .collect();

    let mut infos: Vec<BufferInfo> = buffers
        .iter()
        .enumerate()
        .map(|(i, b)| BufferInfo {
            instance: b.info.id.clone(),
            bytes: b.info.bytes,
            start: lifetimes[i].start,
            end: lifetimes[i].end,
            slot: slot_of[i],
            aliases: aliases[i].map(|p| buffers[p].info.id.clone()),
            pinned: b.pinned,
        })
        .collect();
    let display = |id: &str| &buffers[by_id[id]].info.display;
    infos.sort_by(|a, b| {
        (a.start, display(&a.instance), &a.instance).cmp(&(
            b.start,
            display(&b.instance),
            &b.instance,
        ))
    });

    BufferAxis {
        axis,
        assignment_source: AssignmentSource::Simulated,
        units: units.len(),
        total_bytes: buffers.iter().map(|b| b.info.bytes).sum(),
        peak_live_bytes: live_bytes.iter().copied().max().unwrap_or(0),
        slot_bytes: slots.iter().sum(),
        slot_count: slots.len(),
        slots,
        buffers: infos,
        live_bytes,
    }
}

/// `start` is the unit a buffer is written in, `end` the last unit one of its readers runs in.
fn lifetimes(buffers: &[Buffer], unit_of: &[usize], producers: &[Vec<usize>]) -> Vec<Lifetime> {
    let mut end: Vec<Option<usize>> = vec![None; buffers.len()];
    for (reader, ps) in producers.iter().enumerate() {
        for p in ps {
            end[*p] = Some(end[*p].map_or(unit_of[reader], |e: usize| e.max(unit_of[reader])));
        }
    }
    buffers
        .iter()
        .enumerate()
        .map(|(i, b)| Lifetime {
            start: unit_of[i],
            // No reader can only happen for the entry output in a valid closure; otherwise the
            // buffer is dead right after it is written.
            end: if b.pinned {
                None
            } else {
                Some(end[i].unwrap_or(unit_of[i]))
            },
        })
        .collect()
}

/// Greedy slot assignment (dynamic storage allocation heuristic), with in-place aliasing.
/// Returns, per buffer, its slot and the producer it aliased; and the slot sizes.
fn assign(
    buffers: &[Buffer],
    lifetimes: &[Lifetime],
    producers: &[Vec<usize>],
) -> (Vec<usize>, Vec<Option<usize>>, Vec<usize>) {
    let bytes = |i: usize| buffers[i].info.bytes;
    let mut order: Vec<usize> = (0..buffers.len()).collect();
    order.sort_by(|&a, &b| {
        (
            lifetimes[a].start,
            std::cmp::Reverse(bytes(a)),
            &buffers[a].info.display,
        )
            .cmp(&(
                lifetimes[b].start,
                std::cmp::Reverse(bytes(b)),
                &buffers[b].info.display,
            ))
    });

    let mut slot_of: Vec<Option<usize>> = vec![None; buffers.len()];
    let mut aliases: Vec<Option<usize>> = vec![None; buffers.len()];
    let mut sizes: Vec<usize> = Vec::new();
    let mut occupants: Vec<Vec<usize>> = Vec::new();

    let free_for = |occupants: &[usize], b: usize, except: Option<usize>| {
        occupants
            .iter()
            .filter(|o| Some(**o) != except)
            .all(|o| !lifetimes[*o].overlaps(&lifetimes[b]))
    };

    for b in order {
        let me = lifetimes[b];

        // In-place aliasing: take over the slot of an input whose last reader is `b`.
        let alias = producers[b]
            .iter()
            .copied()
            .filter(|&p| {
                let slot = slot_of[p].expect("producers are assigned first");
                lifetimes[p].end == Some(me.start)
                    && alias_compatible(&buffers[b], &buffers[p])
                    // No other reader runs in the same unit (concurrent readers would race).
                    && producers
                        .iter()
                        .enumerate()
                        .filter(|(r, ps)| ps.contains(&p) && lifetimes[*r].start == me.start)
                        .count()
                        == 1
                    && bytes(b) <= sizes[slot]
                    && free_for(&occupants[slot], b, Some(p))
            })
            .max_by(|&x, &y| {
                (bytes(x), std::cmp::Reverse(&buffers[x].info.display))
                    .cmp(&(bytes(y), std::cmp::Reverse(&buffers[y].info.display)))
            });

        let slot = if let Some(p) = alias {
            aliases[b] = Some(p);
            slot_of[p].unwrap()
        } else {
            let free: Vec<usize> = (0..sizes.len())
                .filter(|s| free_for(&occupants[*s], b, None))
                .collect();
            let best_fit = free
                .iter()
                .copied()
                .filter(|s| sizes[*s] >= bytes(b))
                .min_by_key(|s| (sizes[*s], *s));
            let largest = free
                .iter()
                .copied()
                .max_by_key(|s| (sizes[*s], std::cmp::Reverse(*s)));
            match best_fit.or(largest) {
                Some(s) => {
                    sizes[s] = sizes[s].max(bytes(b));
                    s
                }
                None => {
                    sizes.push(bytes(b));
                    occupants.push(Vec::new());
                    sizes.len() - 1
                }
            }
        };
        slot_of[b] = Some(slot);
        occupants[slot].push(b);
    }

    (
        slot_of.into_iter().map(Option::unwrap).collect(),
        aliases,
        sizes,
    )
}

/// Conditions 2 and 3 of in-place aliasing: every read of `input` by `reader` is un-indexed and
/// on the reader's own grid, so each output cell depends only on the same input cell.
fn alias_compatible(reader: &Buffer, input: &Buffer) -> bool {
    reader
        .info
        .inputs
        .iter()
        .filter(|e| e.producer == input.info.id)
        .all(|e| !e.indexed && !e.resampled)
}

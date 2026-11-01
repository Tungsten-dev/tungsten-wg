//! Per-entry schedules (SPEC.md §6.3). Execution order is the information, so nothing here is
//! re-sorted by name.

use tungsten_interface::Instance;

use super::Ctx;
use crate::model::Schedule;

/// An entry's pruned waves, and the same instances flattened into CPU steps.
pub(crate) struct EntrySchedule {
    /// Wave by wave, and within a wave in the order `waves_for` returns: the order the Rust and
    /// Java orchestration launch kernels in.
    pub steps: Vec<Instance>,
    pub waves: Vec<Vec<Instance>>,
}

impl EntrySchedule {
    pub fn of(ctx: &Ctx, entry: &Instance) -> Self {
        let waves = ctx.interface.waves_for(entry);
        let steps = waves.iter().flatten().copied().collect();
        Self { steps, waves }
    }

    pub fn to_model(&self, ctx: &Ctx) -> Schedule {
        Schedule {
            steps: self.steps.iter().map(|i| ctx.instance_id(i)).collect(),
            waves: self
                .waves
                .iter()
                .map(|w| w.iter().map(|i| ctx.instance_id(i)).collect())
                .collect(),
        }
    }
}

//! Buffer charts (`buffers.{cpu,gpu}.svg`) and the matching table (`buffers.txt`), SPEC.md §7.4.
//!
//! Hand-written SVG with fixed light colors and sizes. The width grows with the number of time
//! units and is never scaled down.

use std::fmt::Write;

use super::Ctx;
use super::format::{bytes, xml};
use crate::model::{Axis, BufferAxis, EntryInfo};

/// 12-color qualitative palette (ColorBrewer Set3), cycled by slot index.
const PALETTE: [&str; 12] = [
    "#8dd3c7", "#ffffb3", "#bebada", "#fb8072", "#80b1d3", "#fdb462", "#b3de69", "#fccde5",
    "#d9d9d9", "#bc80bd", "#ccebc5", "#ffed6f",
];

const ROW: usize = 22;
const COL: usize = 36;
const CHAR: usize = 7;
const MARGIN: usize = 16;
const FOOTER: usize = 120;
/// Room right of the grid for the pinned arrow and the byte counts.
const RIGHT: usize = 110;

fn axis_name(axis: Axis) -> &'static str {
    match axis {
        Axis::Cpu => "CPU steps",
        Axis::Gpu => "GPU waves",
    }
}

/// Column header of time unit `u`: the step's instance, or `wave N`.
fn unit_label(ctx: &Ctx, entry: &EntryInfo, axis: &BufferAxis, u: usize) -> String {
    match axis.axis {
        Axis::Cpu => ctx.display(&entry.schedule.steps[u]).to_string(),
        Axis::Gpu => format!("wave {u}"),
    }
}

pub(crate) fn chart(ctx: &Ctx, entry: &EntryInfo, axis: &BufferAxis) -> String {
    let units = axis.units;
    let labels: Vec<String> = (0..units)
        .map(|u| unit_label(ctx, entry, axis, u))
        .collect();
    let names: Vec<&str> = axis
        .buffers
        .iter()
        .map(|b| ctx.display(&b.instance))
        .collect();

    let left = MARGIN + names.iter().map(|n| n.chars().count()).max().unwrap_or(0) * CHAR + 12;
    let header_lines = 3;
    // Column labels are drawn at -45°; leave room for the longest.
    let label_room =
        labels.iter().map(|l| l.chars().count()).max().unwrap_or(0) * CHAR * 5 / 7 + 12;
    let grid_top = MARGIN + header_lines * 18 + label_room;
    let grid_w = units * COL;
    let grid_h = axis.buffers.len() * ROW;
    let width = left + grid_w + RIGHT + MARGIN;
    let footer_top = grid_top + grid_h + 30;
    let height = footer_top + FOOTER + MARGIN + 20;

    let mut s = String::new();
    let _ = writeln!(
        s,
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{width}" height="{height}" viewBox="0 0 {width} {height}" font-family="sans-serif" font-size="12">"#
    );
    let _ = writeln!(
        s,
        r##"<rect x="0" y="0" width="{width}" height="{height}" fill="#ffffff"/>"##
    );

    // Header.
    let lines = [
        format!("{} — {}", entry.display, axis_name(axis.axis)),
        format!(
            "total {} · peak live {} · simulated slots: {}, {}",
            bytes(axis.total_bytes),
            bytes(axis.peak_live_bytes),
            axis.slot_count,
            bytes(axis.slot_bytes)
        ),
    ];
    for (n, line) in lines.iter().enumerate() {
        let weight = if n == 0 { r#" font-weight="bold""# } else { "" };
        let _ = writeln!(
            s,
            r##"<text x="{MARGIN}" y="{}" fill="#000000"{weight}>{}</text>"##,
            MARGIN + 14 + n * 18,
            xml(line)
        );
    }

    // Columns: alternating background and a rotated label.
    for (u, label) in labels.iter().enumerate() {
        let x = left + u * COL;
        let fill = if u % 2 == 0 { "#f4f4f4" } else { "#ffffff" };
        let _ = writeln!(
            s,
            r##"<rect x="{x}" y="{grid_top}" width="{COL}" height="{grid_h}" fill="{fill}"><title>{}: {} live</title></rect>"##,
            xml(label),
            bytes(axis.live_bytes[u])
        );
        let (tx, ty) = (x + COL / 2, grid_top - 6);
        let _ = writeln!(
            s,
            r##"<text x="{tx}" y="{ty}" transform="rotate(-45 {tx} {ty})" fill="#333333">{}</text>"##,
            xml(label)
        );
    }

    // Rows and bars.
    for (row, (b, name)) in axis.buffers.iter().zip(&names).enumerate() {
        let y = grid_top + row * ROW;
        let _ = writeln!(
            s,
            r##"<text x="{}" y="{}" text-anchor="end" fill="#000000">{}</text>"##,
            left - 8,
            y + ROW - 7,
            xml(name)
        );
        let x0 = left + b.start * COL + 2;
        let x1 = match b.end {
            Some(end) => left + (end + 1) * COL - 2,
            None => left + grid_w + 18,
        };
        let color = PALETTE[b.slot % PALETTE.len()];
        let end = b.end.map_or("∞ (pinned)".to_string(), |e| e.to_string());
        let mut title = format!(
            "{name}\n{} bytes, slot {}, [{}, {end}]",
            b.bytes, b.slot, b.start
        );
        if let Some(a) = &b.aliases {
            title.push_str(&format!("\nin place of {}", ctx.display(a)));
        }
        let _ = writeln!(
            s,
            r##"<rect x="{x0}" y="{}" width="{}" height="{}" rx="3" fill="{color}" stroke="#555555"><title>{}</title></rect>"##,
            y + 3,
            x1 - x0,
            ROW - 6,
            xml(&title)
        );
        if b.end.is_none() {
            let (ax, ay) = (x1, y + ROW / 2);
            let _ = writeln!(
                s,
                r##"<polygon points="{ax},{} {},{ay} {ax},{}" fill="#555555"/>"##,
                ay - 6,
                ax + 8,
                ay + 6
            );
        }
        let mut marker = String::new();
        if let Some(a) = &b.aliases {
            marker = format!(
                r##"<tspan fill="#000000"><title>in place of {}</title>↪ </tspan>"##,
                xml(ctx.display(a))
            );
        }
        let _ = writeln!(
            s,
            r##"<text x="{}" y="{}" fill="#000000">{marker}{}</text>"##,
            x0 + 4,
            y + ROW - 7,
            b.slot
        );
        let _ = writeln!(
            s,
            r##"<text x="{}" y="{}" fill="#333333">{}</text>"##,
            left + grid_w + 30,
            y + ROW - 7,
            bytes(b.bytes)
        );
    }

    // Footer: live bytes per unit as a step chart, with the peak and slot totals.
    let top = axis.peak_live_bytes.max(axis.slot_bytes).max(1);
    let y_of = |v: usize| footer_top + FOOTER - v * FOOTER / top;
    let _ = writeln!(
        s,
        r##"<line x1="{left}" y1="{}" x2="{}" y2="{}" stroke="#999999"/>"##,
        footer_top + FOOTER,
        left + grid_w,
        footer_top + FOOTER
    );
    if units > 0 {
        let mut d = format!("M{left},{}", y_of(axis.live_bytes[0]));
        for (u, v) in axis.live_bytes.iter().enumerate() {
            let _ = write!(d, " H{} V{}", left + u * COL, y_of(*v));
            let _ = write!(d, " H{}", left + (u + 1) * COL);
        }
        let _ = writeln!(
            s,
            r##"<path d="{d}" fill="none" stroke="#1f78b4" stroke-width="2"/>"##
        );
    }
    for (value, label, color) in [
        (axis.peak_live_bytes, "peak live", "#e31a1c"),
        (axis.slot_bytes, "slots", "#33a02c"),
    ] {
        let y = y_of(value);
        let _ = writeln!(
            s,
            r##"<line x1="{left}" y1="{y}" x2="{}" y2="{y}" stroke="{color}" stroke-dasharray="4 3"><title>{label}: {value} bytes</title></line>"##,
            left + grid_w
        );
        let _ = writeln!(
            s,
            r##"<text x="{}" y="{}" fill="{color}">{label} {}</text>"##,
            left + grid_w + 6,
            y + 4,
            bytes(value)
        );
    }
    let _ = writeln!(
        s,
        r##"<text x="{}" y="{}" text-anchor="end" fill="#333333">live bytes</text>"##,
        left - 8,
        footer_top + FOOTER / 2
    );
    s.push_str("</svg>\n");
    s
}

/// `buffers.txt`: both axes as fixed-width tables, then live bytes per unit and the totals.
pub(crate) fn table(ctx: &Ctx, entry: &EntryInfo) -> String {
    let mut s = format!("buffers: {}\n", entry.display);
    for axis in [&entry.buffers.cpu, &entry.buffers.gpu] {
        let _ = writeln!(s, "\n== {} ==\n", axis_name(axis.axis));
        let header = ["buffer", "bytes", "start", "end", "slot", "alias", "pinned"];
        let rows: Vec<[String; 7]> = axis
            .buffers
            .iter()
            .map(|b| {
                [
                    ctx.display(&b.instance).to_string(),
                    b.bytes.to_string(),
                    b.start.to_string(),
                    b.end.map_or("∞".to_string(), |e| e.to_string()),
                    b.slot.to_string(),
                    b.aliases
                        .as_deref()
                        .map_or("-".to_string(), |a| ctx.display(a).to_string()),
                    if b.pinned { "yes" } else { "no" }.to_string(),
                ]
            })
            .collect();
        let widths: Vec<usize> = (0..7)
            .map(|c| {
                rows.iter()
                    .map(|r| r[c].chars().count())
                    .chain([header[c].len()])
                    .max()
                    .unwrap()
            })
            .collect();
        let line = |cells: &[&str]| {
            let padded: Vec<String> = cells
                .iter()
                .zip(&widths)
                .enumerate()
                .map(|(c, (v, w))| {
                    // Text left-aligned, numbers right-aligned.
                    if c == 0 || c == 5 {
                        format!("{v:<w$}")
                    } else {
                        format!("{v:>w$}")
                    }
                })
                .collect();
            padded.join(" | ").trim_end().to_string()
        };
        let _ = writeln!(s, "{}", line(&header));
        let _ = writeln!(
            s,
            "{}",
            widths
                .iter()
                .map(|w| "-".repeat(*w))
                .collect::<Vec<_>>()
                .join("-+-")
        );
        for r in &rows {
            let cells: Vec<&str> = r.iter().map(String::as_str).collect();
            let _ = writeln!(s, "{}", line(&cells));
        }
        let _ = writeln!(
            s,
            "\nlive bytes per {}:",
            match axis.axis {
                Axis::Cpu => "step",
                Axis::Gpu => "wave",
            }
        );
        for (u, v) in axis.live_bytes.iter().enumerate() {
            let _ = writeln!(s, "  {u:>3}  {v:>12}  {}", unit_label(ctx, entry, axis, u));
        }
        let _ = writeln!(
            s,
            "\ntotal {} bytes, peak live {} bytes, simulated slots {} ({} bytes: {:?})",
            axis.total_bytes, axis.peak_live_bytes, axis.slot_count, axis.slot_bytes, axis.slots
        );
    }
    s
}

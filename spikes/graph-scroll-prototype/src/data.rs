//! Fake graph layout. Shape calibrated on linux-graph (all refs, topo order):
//! 7.5% merges, 6.5% lanes closing, and far more active lanes than any sane cap
//! (mean 294 edges per row), so every lane column is occupied on every row.
//! `Rows` is the plan's format. `Delta` is the lever: events plus keyframes.

pub const NONE: u16 = u16::MAX;
const KEYFRAME_EVERY: usize = 256;

#[derive(Clone, Copy)]
pub struct Event {
    pub node: u16,
    pub colour: u16,
    pub merge_to: u16,
    pub close_from: u16,
}

#[derive(Clone, Copy)]
pub struct Edge {
    pub top: u16,
    pub bottom: u16,
    pub colour: u16,
}

#[derive(Clone, Copy)]
pub struct Head {
    edge_start: u32,
    node: u16,
    colour: u16,
}

#[derive(Default)]
pub struct Visible {
    pub nodes: Vec<(u32, u16, u16)>,
    pub edges: Vec<(u32, Edge)>,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Format {
    Rows,
    Delta,
}

pub enum Store {
    Rows { heads: Vec<Head>, edges: Vec<Edge> },
    Delta { events: Vec<Event>, keyframes: Vec<u16>, cap: usize },
}

fn fresh_colour(row: usize) -> u16 {
    ((row as u64).wrapping_mul(2654435761) >> 11) as u16 & 0x7fff
}

fn apply(row: usize, e: &Event, lanes: &mut [u16]) {
    if e.merge_to != NONE {
        lanes[e.merge_to as usize] = fresh_colour(row);
    }
    if e.close_from != NONE {
        lanes[e.close_from as usize] = fresh_colour(row ^ 0x5555);
    }
}

fn event_at(row: usize, cap: usize, lanes: &[u16]) -> Event {
    // SplitMix64 of the row, so any prefix is identical to the full data.
    let mut z = (row as u64).wrapping_add(1).wrapping_mul(0x9E3779B97F4A7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB);
    z ^= z >> 31;
    let node = (z % cap as u64) as usize;
    let other = (((z >> 20) % (cap as u64 - 1)) as usize + node + 1) % cap;
    let kind = (z >> 44) % 1000;
    Event {
        node: node as u16,
        colour: lanes[node],
        merge_to: if kind < 75 { other as u16 } else { NONE },
        close_from: if (75..140).contains(&kind) { other as u16 } else { NONE },
    }
}

fn expand(row: u32, e: &Event, lanes: &[u16], mut push: impl FnMut(u32, Edge)) {
    for (c, &colour) in lanes.iter().enumerate() {
        push(row, Edge { top: c as u16, bottom: c as u16, colour });
    }
    if e.merge_to != NONE {
        push(row, Edge { top: e.node, bottom: e.merge_to, colour: e.colour });
    }
    if e.close_from != NONE {
        push(row, Edge { top: e.close_from, bottom: e.node, colour: lanes[e.close_from as usize] });
    }
}

impl Store {
    pub fn build(format: Format, rows: usize, cap: usize) -> Store {
        let mut lanes: Vec<u16> = (0..cap).map(fresh_colour).collect();
        match format {
            Format::Rows => {
                let mut heads = Vec::with_capacity(rows);
                let mut edges = Vec::with_capacity(rows * cap + rows / 6);
                for row in 0..rows {
                    let e = event_at(row, cap, &lanes);
                    heads.push(Head { edge_start: edges.len() as u32, node: e.node, colour: e.colour });
                    expand(row as u32, &e, &lanes, |_, edge| edges.push(edge));
                    apply(row, &e, &mut lanes);
                }
                Store::Rows { heads, edges }
            }
            Format::Delta => {
                let mut events = Vec::with_capacity(rows);
                let mut keyframes = Vec::with_capacity((rows / KEYFRAME_EVERY + 1) * cap);
                for row in 0..rows {
                    if row % KEYFRAME_EVERY == 0 {
                        keyframes.extend_from_slice(&lanes);
                    }
                    let e = event_at(row, cap, &lanes);
                    events.push(e);
                    apply(row, &e, &mut lanes);
                }
                Store::Delta { events, keyframes, cap }
            }
        }
    }

    pub fn len(&self) -> usize {
        match self {
            Store::Rows { heads, .. } => heads.len(),
            Store::Delta { events, .. } => events.len(),
        }
    }

    pub fn bytes(&self) -> usize {
        use std::mem::size_of_val as s;
        match self {
            Store::Rows { heads, edges } => s(&heads[..]) + s(&edges[..]),
            Store::Delta { events, keyframes, .. } => s(&events[..]) + s(&keyframes[..]),
        }
    }

    pub fn read(&self, range: std::ops::Range<usize>, out: &mut Visible) {
        out.nodes.clear();
        out.edges.clear();
        let end = range.end.min(self.len());
        if range.start >= end {
            return;
        }
        match self {
            Store::Rows { heads, edges } => {
                for row in range.start..end {
                    let h = heads[row];
                    let stop = heads.get(row + 1).map_or(edges.len(), |n| n.edge_start as usize);
                    out.nodes.push((row as u32, h.node, h.colour));
                    for e in &edges[h.edge_start as usize..stop] {
                        out.edges.push((row as u32, *e));
                    }
                }
            }
            Store::Delta { events, keyframes, cap } => {
                let key = range.start / KEYFRAME_EVERY;
                let mut lanes = [0u16; 512];
                let lanes = &mut lanes[..*cap];
                lanes.copy_from_slice(&keyframes[key * cap..(key + 1) * cap]);
                for row in key * KEYFRAME_EVERY..end {
                    let e = &events[row];
                    if row >= range.start {
                        out.nodes.push((row as u32, e.node, e.colour));
                        expand(row as u32, e, lanes, |r, edge| out.edges.push((r, edge)));
                    }
                    apply(row, e, lanes);
                }
            }
        }
    }
}

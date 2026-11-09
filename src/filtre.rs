// The palm filter itself, without any device: events in, events out.
// Same rules and same output order as the old Python palm-guard.

use std::collections::{BTreeMap, HashSet};

pub const EV_SYN: u16 = 0x00;
pub const EV_KEY: u16 = 0x01;
pub const EV_ABS: u16 = 0x03;
pub const SYN_REPORT: u16 = 0;
pub const SYN_DROPPED: u16 = 3;

pub const ABS_X: u16 = 0x00;
pub const ABS_Y: u16 = 0x01;
pub const ABS_PRESSURE: u16 = 0x18;
pub const ABS_MT_SLOT: u16 = 0x2f;
pub const ABS_MT_TOUCH_MAJOR: u16 = 0x30;
pub const ABS_MT_POSITION_X: u16 = 0x35;
pub const ABS_MT_POSITION_Y: u16 = 0x36;
pub const ABS_MT_TOOL_TYPE: u16 = 0x37;
pub const ABS_MT_TRACKING_ID: u16 = 0x39;
pub const ABS_MT_PRESSURE: u16 = 0x3a;
pub const ABS_MT_TOOL_Y: u16 = 0x3d;

pub const BTN_TOOL_PEN: u16 = 0x140;
pub const BTN_TOOL_RUBBER: u16 = 0x141;
pub const BTN_TOOL_FINGER: u16 = 0x145;
pub const BTN_TOUCH: u16 = 0x14a;
const TOOLS: [u16; 5] = [BTN_TOOL_FINGER, 0x14d, 0x14e, 0x14f, 0x148]; // finger, double, triple, quad, quint tap

const FINGER: i32 = 0;
const PALM: i32 = 2;

// settings
pub const EDGE_LEFT_MM: f64 = 20.0; // dead zone on the left edge (right, top, bottom: 0)
const XOURNAL_ONLY: bool = true; // dead zone only while Xournal++ is active
const PEN_BLOCKS: bool = true; // pen near the screen = touch off
const PEN_DELAY: f64 = 0.3; // seconds after the pen leaves
const PALM_MM: f64 = 0.0; // wider touch = palm (0 = off)
pub const SCREEN_MM: (f64, f64) = (301.5, 188.5); // 14" 16:10 screen

pub struct Geo {
    pub x0: f64,
    pub y0: f64,
    pub ux: f64,
    pub uy: f64,
    pub w: f64,
    pub h: f64,
}

impl Geo {
    /// from the min, max and resolution of ABS_MT_POSITION_X / _Y
    pub fn new(x: (i32, i32, i32), y: (i32, i32, i32)) -> Geo {
        let unit = |(min, max, res): (i32, i32, i32), mm: f64| if res != 0 { res as f64 } else { (max - min) as f64 / mm };
        let (ux, uy) = (unit(x, SCREEN_MM.0), unit(y, SCREEN_MM.1));
        Geo { x0: x.0 as f64, y0: y.0 as f64, ux, uy, w: (x.1 - x.0) as f64 / ux, h: (y.1 - y.0) as f64 / uy }
    }

    pub fn mm(&self, x: i32, y: i32) -> (f64, f64) {
        ((x as f64 - self.x0) / self.ux, (y as f64 - self.y0) / self.uy)
    }

    fn at_edge(&self, x: i32, y: i32) -> bool {
        let (mx, my) = self.mm(x, y);
        mx < EDGE_LEFT_MM || mx > self.w || my < 0.0 || my > self.h
    }
}

/// a small map that keeps insertion order (like a Python dict)
#[derive(Default, Clone)]
struct Vals(Vec<(u16, i32)>);

impl Vals {
    fn get(&self, c: u16) -> Option<i32> {
        self.0.iter().find(|(k, _)| *k == c).map(|(_, v)| *v)
    }
    fn set(&mut self, c: u16, v: i32) {
        match self.0.iter_mut().find(|(k, _)| *k == c) {
            Some(e) => e.1 = v,
            None => self.0.push((c, v)),
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum State {
    None,
    Ok,
    Blocked,
}

#[derive(Default)]
struct Slot {
    active: bool,
    state: Option<bool>, // None / Some(true) = ok / Some(false) = blocked
    vals: Vals,
    changed: Vals,
    start: bool,
    end: bool,
}

impl Slot {
    fn state(&self) -> State {
        match self.state {
            None => State::None,
            Some(true) => State::Ok,
            Some(false) => State::Blocked,
        }
    }
}

pub trait Out {
    fn emit(&mut self, t: u16, c: u16, v: i32);
    fn syn(&mut self);
}

pub struct Filter<O: Out> {
    pub out: Option<O>,
    pub geo: Geo,
    keys: HashSet<u16>,
    abss: HashSet<u16>,
    pub test: bool,
    slots: BTreeMap<i32, Slot>,
    cur: i32,
    vid: i32,
    lost: bool,
    pen_near: bool,
    pen_left: f64,
    pub xournal: bool,
}

impl<O: Out> Filter<O> {
    pub fn new(out: Option<O>, geo: Geo, keys: HashSet<u16>, abss: HashSet<u16>, test: bool) -> Self {
        Filter { out, geo, keys, abss, test, slots: BTreeMap::new(), cur: 0, vid: 0, lost: false, pen_near: false, pen_left: -100.0, xournal: false }
    }

    fn emit(&mut self, t: u16, c: u16, v: i32) {
        if let Some(o) = self.out.as_mut() {
            o.emit(t, c, v);
        }
    }

    fn syn(&mut self) {
        if let Some(o) = self.out.as_mut() {
            o.syn();
        }
    }

    // screen
    pub fn screen(&mut self, t: u16, c: u16, v: i32, now: f64) {
        if t == EV_SYN {
            if c == SYN_DROPPED {
                self.lost = true;
            } else if c == SYN_REPORT {
                if self.lost {
                    self.lost = false;
                    self.cancel_all();
                    for s in self.slots.values_mut() {
                        s.state = Some(false);
                        s.start = false;
                        s.end = false;
                        s.changed = Vals::default();
                    }
                } else {
                    self.frame(now);
                }
            }
            return;
        }
        if self.lost || t != EV_ABS {
            return;
        }
        if c == ABS_MT_SLOT {
            self.cur = v;
            return;
        }
        let s = self.slots.entry(self.cur).or_default();
        if c == ABS_MT_TRACKING_ID {
            if v < 0 {
                if s.start {
                    s.start = false;
                } else {
                    s.end = true;
                }
                s.active = false;
            } else {
                if s.active {
                    s.end = true;
                }
                s.active = true;
                s.start = true;
            }
            return;
        }
        if (ABS_MT_TOUCH_MAJOR..=ABS_MT_TOOL_Y).contains(&c) {
            s.vals.set(c, v);
            s.changed.set(c, v);
        }
    }

    fn reason(&self, s: &Slot, now: f64, ongoing: bool) -> Option<&'static str> {
        if s.vals.get(ABS_MT_TOOL_TYPE).unwrap_or(FINGER) == PALM {
            return Some("paume");
        }
        if PALM_MM != 0.0 && s.vals.get(ABS_MT_TOUCH_MAJOR).unwrap_or(0) as f64 / self.geo.ux > PALM_MM {
            return Some("trop large");
        }
        if ongoing {
            return None;
        }
        if PEN_BLOCKS && (self.pen_near || now - self.pen_left < PEN_DELAY) {
            return Some("stylet proche");
        }
        let edges = self.xournal || !XOURNAL_ONLY;
        if let (true, Some(x), Some(y)) = (edges, s.vals.get(ABS_MT_POSITION_X), s.vals.get(ABS_MT_POSITION_Y)) {
            if self.geo.at_edge(x, y) {
                return Some("bord");
            }
        }
        None
    }

    fn log(&self, s: &Slot, why: Option<&str>) {
        if !self.test {
            return;
        }
        let (mx, my) = self.geo.mm(s.vals.get(ABS_MT_POSITION_X).unwrap_or(0), s.vals.get(ABS_MT_POSITION_Y).unwrap_or(0));
        let size = s.vals.get(ABS_MT_TOUCH_MAJOR).map_or("?".to_string(), |m| format!("{:.0}mm", m as f64 / self.geo.ux));
        let state = why.map_or("OK".to_string(), |w| format!("BLOQUEE ({w})"));
        println!("Touche x={mx:.0}mm y={my:.0}mm taille={size} -> {state}");
    }

    fn frame(&mut self, now: f64) {
        let mut lift = Vec::new();
        let ids: Vec<i32> = self.slots.keys().copied().collect();
        for n in ids {
            let mut s = self.slots.remove(&n).unwrap();
            if s.end {
                if s.state() == State::Ok {
                    self.emit(EV_ABS, ABS_MT_SLOT, n);
                    self.emit(EV_ABS, ABS_MT_TRACKING_ID, -1);
                }
                s.state = None;
            }
            if s.start {
                let why = self.reason(&s, now, false);
                self.log(&s, why);
                if why.is_some() {
                    s.state = Some(false);
                } else {
                    s.state = Some(true);
                    self.vid = (self.vid + 1) % 60000;
                    self.emit(EV_ABS, ABS_MT_SLOT, n);
                    self.emit(EV_ABS, ABS_MT_TOOL_TYPE, FINGER);
                    self.emit(EV_ABS, ABS_MT_TRACKING_ID, self.vid);
                    for &(c, v) in &s.vals.0 {
                        if self.abss.contains(&c) && c != ABS_MT_TOOL_TYPE {
                            self.emit(EV_ABS, c, v);
                        }
                    }
                }
            } else if s.state() == State::Ok && !s.changed.0.is_empty() {
                self.emit(EV_ABS, ABS_MT_SLOT, n);
                if self.reason(&s, now, true).is_some() {
                    self.emit(EV_ABS, ABS_MT_TOOL_TYPE, PALM);
                    s.state = Some(false);
                    lift.push(n);
                } else {
                    for &(c, v) in &s.changed.0 {
                        if self.abss.contains(&c) && c != ABS_MT_TOOL_TYPE {
                            self.emit(EV_ABS, c, v);
                        }
                    }
                }
            }
            s.start = false;
            s.end = false;
            s.changed = Vals::default();
            self.slots.insert(n, s);
        }
        self.pointer();
        self.syn();
        if !lift.is_empty() {
            self.lift(&lift);
        }
    }

    fn pointer(&mut self) {
        let ok: Vec<i32> = self.slots.iter().filter(|(_, s)| s.state() == State::Ok).map(|(n, _)| *n).collect();
        let nb = ok.len();
        if self.keys.contains(&BTN_TOUCH) {
            self.emit(EV_KEY, BTN_TOUCH, (nb > 0) as i32);
        }
        for (i, k) in TOOLS.iter().enumerate() {
            if self.keys.contains(k) {
                let on = nb == i + 1 || (i == 4 && nb > 5);
                self.emit(EV_KEY, *k, on as i32);
            }
        }
        if let Some(first) = ok.first() {
            let vals = self.slots[first].vals.clone();
            for (mt, st) in [(ABS_MT_POSITION_X, ABS_X), (ABS_MT_POSITION_Y, ABS_Y), (ABS_MT_PRESSURE, ABS_PRESSURE)] {
                if let (true, Some(v)) = (self.abss.contains(&st), vals.get(mt)) {
                    self.emit(EV_ABS, st, v);
                }
            }
        }
    }

    pub fn cancel_all(&mut self) {
        let ok: Vec<i32> = self.slots.iter().filter(|(_, s)| s.state() == State::Ok).map(|(n, _)| *n).collect();
        if ok.is_empty() {
            return;
        }
        for &n in &ok {
            self.emit(EV_ABS, ABS_MT_SLOT, n);
            self.emit(EV_ABS, ABS_MT_TOOL_TYPE, PALM);
            self.slots.get_mut(&n).unwrap().state = Some(false);
        }
        self.syn();
        self.lift(&ok);
    }

    fn lift(&mut self, ns: &[i32]) {
        for &n in ns {
            self.emit(EV_ABS, ABS_MT_SLOT, n);
            self.emit(EV_ABS, ABS_MT_TRACKING_ID, -1);
        }
        self.pointer();
        self.syn();
    }

    // pen
    pub fn pen(&mut self, t: u16, c: u16, v: i32, now: f64) {
        if t != EV_KEY || (c != BTN_TOOL_PEN && c != BTN_TOOL_RUBBER) {
            return;
        }
        if v != 0 {
            if !self.pen_near {
                self.pen_near = true;
                if self.test {
                    println!("Stylet proche");
                }
                if PEN_BLOCKS {
                    self.cancel_all();
                }
            }
        } else if self.pen_near {
            self.pen_near = false;
            self.pen_left = now;
            if self.test {
                println!("Stylet loin");
            }
        }
    }
}

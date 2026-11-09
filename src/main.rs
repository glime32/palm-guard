// palm-guard: palm rejection for the touchscreen (KDE Wayland). Rust port.
//  - blocks a touch that starts on the left edge, only while Xournal++ is active
//  - turns touch off while the pen is near (+ a short delay)
//  - blocks a touch the screen marks as a palm
// It grabs the real touchscreen and replays the allowed touches on a virtual one.
// Asleep in poll() between events: no timer at all.
//
// Test: sudo systemctl stop palm-guard ; sudo palm-guard --test
// Replay (to compare with another implementation): palm-guard --rejouer FILE

mod filtre;

use filtre::*;
use std::collections::HashSet;
use std::ffi::CString;
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::os::unix::io::{AsRawFd, RawFd};

const NAME: &str = "Palm Guard Touchscreen";
const FIFO: &str = "/run/palm-guard.fifo"; // KWin writes there whether Xournal++ is active

const EV_FF: u16 = 0x15;
const EV_MAX: u16 = 0x1f;
const KEY_MAX: usize = 0x2ff;
const ABS_MAX: usize = 0x3f;
const INPUT_PROP_DIRECT: usize = 1;

// ioctl numbers (asm-generic)
const fn ioc(dir: u64, ty: u8, nr: u64, size: u64) -> u64 {
    (dir << 30) | (size << 16) | ((ty as u64) << 8) | nr
}
const R: u64 = 2;
const W: u64 = 1;
const fn eviocgbit(ev: u64, len: u64) -> u64 {
    ioc(R, b'E', 0x20 + ev, len)
}
const EVIOCGID: u64 = ioc(R, b'E', 0x02, 8);
const fn eviocgname(len: u64) -> u64 {
    ioc(R, b'E', 0x06, len)
}
const fn eviocgprop(len: u64) -> u64 {
    ioc(R, b'E', 0x09, len)
}
const fn eviocgabs(abs: u64) -> u64 {
    ioc(R, b'E', 0x40 + abs, 24)
}
const EVIOCGRAB: u64 = ioc(W, b'E', 0x90, 4);
const UI_DEV_CREATE: u64 = ioc(0, b'U', 1, 0);
const UI_DEV_SETUP: u64 = ioc(W, b'U', 3, 92);
const UI_ABS_SETUP: u64 = ioc(W, b'U', 4, 28);
const UI_SET_EVBIT: u64 = ioc(W, b'U', 100, 4);
const UI_SET_ABSBIT: u64 = ioc(W, b'U', 103, 4);
const UI_SET_PROPBIT: u64 = ioc(W, b'U', 110, 4);
// key, rel, abs, msc, led, snd, ff, -, sw: UI_SET_KEYBIT + offset
const UI_SET_BIT: [(u16, u64); 7] = [(0x01, 101), (0x02, 102), (0x03, 103), (0x04, 104), (0x11, 105), (0x12, 106), (0x05, 109)];

#[repr(C)]
#[derive(Default, Clone, Copy)]
struct AbsInfo {
    value: i32,
    min: i32,
    max: i32,
    fuzz: i32,
    flat: i32,
    res: i32,
}

#[repr(C)]
#[derive(Default, Clone, Copy)]
struct InputId {
    bustype: u16,
    vendor: u16,
    product: u16,
    version: u16,
}

#[repr(C)]
struct UinputSetup {
    id: InputId,
    name: [u8; 80],
    ff_effects_max: u32,
}

#[repr(C)]
struct UinputAbsSetup {
    code: u16,
    absinfo: AbsInfo,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct InputEvent {
    sec: libc::time_t,
    usec: libc::suseconds_t,
    kind: u16,
    code: u16,
    value: i32,
}

fn ioctl<T>(fd: RawFd, req: u64, arg: *mut T) -> i32 {
    unsafe { libc::ioctl(fd, req as libc::c_ulong, arg) }
}

fn bits(fd: RawFd, req: u64, len: usize) -> Vec<usize> {
    let mut buf = vec![0u8; len];
    if ioctl(fd, req, buf.as_mut_ptr()) < 0 {
        return Vec::new();
    }
    (0..len * 8).filter(|i| buf[i / 8] & (1 << (i % 8)) != 0).collect()
}

struct Device {
    file: File,
    name: String,
    props: Vec<usize>,
    types: Vec<usize>,
    keys: Vec<usize>,
    abs: Vec<usize>,
}

impl Device {
    fn open(path: &str) -> Option<Device> {
        let file = OpenOptions::new().read(true).custom_flags(libc::O_NONBLOCK).open(path).ok()?;
        let fd = file.as_raw_fd();
        let mut name = [0u8; 256];
        ioctl(fd, eviocgname(256), name.as_mut_ptr());
        let name = String::from_utf8_lossy(&name[..name.iter().position(|&b| b == 0).unwrap_or(0)]).into_owned();
        Some(Device {
            props: bits(fd, eviocgprop(8), 8),
            types: bits(fd, eviocgbit(0, 4), 4),
            keys: bits(fd, eviocgbit(EV_KEY as u64, (KEY_MAX + 1) as u64 / 8), (KEY_MAX + 1) / 8),
            abs: bits(fd, eviocgbit(EV_ABS as u64, 8), 8),
            name,
            file,
        })
    }

    fn fd(&self) -> RawFd {
        self.file.as_raw_fd()
    }

    fn absinfo(&self, code: u16) -> AbsInfo {
        let mut a = AbsInfo::default();
        ioctl(self.fd(), eviocgabs(code as u64), &mut a);
        a
    }

    fn read_events(&mut self, out: &mut Vec<InputEvent>) -> bool {
        let size = std::mem::size_of::<InputEvent>();
        let mut buf = [0u8; 64 * 24];
        match self.file.read(&mut buf) {
            Ok(n) if n > 0 => {
                for chunk in buf[..n].chunks_exact(size) {
                    out.push(unsafe { std::ptr::read_unaligned(chunk.as_ptr() as *const InputEvent) });
                }
                true
            }
            _ => false,
        }
    }
}

fn find() -> (Option<Device>, Option<Device>) {
    let mut paths: Vec<(u32, String)> = fs::read_dir("/dev/input")
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().into_string().ok()?;
            Some((name.strip_prefix("event")?.parse().ok()?, format!("/dev/input/{name}")))
        })
        .collect();
    paths.sort();
    let (mut screen, mut pen) = (None, None);
    for (_, p) in paths {
        let Some(d) = Device::open(&p) else { continue };
        if !d.props.contains(&INPUT_PROP_DIRECT) || d.name == NAME {
            continue;
        }
        if d.keys.contains(&(BTN_TOOL_PEN as usize)) {
            if pen.is_none() {
                pen = Some(d);
            }
        } else if d.abs.contains(&(ABS_MT_POSITION_X as usize)) && d.abs.contains(&(ABS_MT_SLOT as usize)) && screen.is_none() {
            screen = Some(d);
        }
    }
    (screen, pen)
}

// the virtual touchscreen

struct Uinput {
    file: File,
    buf: Vec<u8>,
}

impl Out for Uinput {
    fn emit(&mut self, t: u16, c: u16, v: i32) {
        let ev = InputEvent { sec: 0, usec: 0, kind: t, code: c, value: v };
        let bytes = unsafe { std::slice::from_raw_parts(&ev as *const InputEvent as *const u8, std::mem::size_of::<InputEvent>()) };
        self.buf.extend_from_slice(bytes);
    }

    // one write per frame instead of one per event
    fn syn(&mut self) {
        self.emit(EV_SYN, SYN_REPORT, 0);
        let _ = self.file.write_all(&self.buf);
        self.buf.clear();
    }
}

fn virtual_screen(dev: &Device) -> std::io::Result<(Uinput, HashSet<u16>, HashSet<u16>)> {
    let file = OpenOptions::new().write(true).custom_flags(libc::O_NONBLOCK).open("/dev/uinput")?;
    let fd = file.as_raw_fd();
    let mut abss = HashSet::new();
    let mut keys = HashSet::new();
    for &t in &dev.types {
        let t = t as u16;
        if t == EV_SYN || t == EV_FF || t > EV_MAX {
            continue;
        }
        ioctl(fd, UI_SET_EVBIT, t as usize as *mut libc::c_void);
        if t == EV_ABS {
            for &c in &dev.abs {
                let c = c as u16;
                if c as usize > ABS_MAX || c == ABS_MT_TOOL_TYPE {
                    continue;
                }
                let mut s = UinputAbsSetup { code: c, absinfo: dev.absinfo(c) };
                ioctl(fd, UI_SET_ABSBIT, c as usize as *mut libc::c_void);
                ioctl(fd, UI_ABS_SETUP, &mut s);
                abss.insert(c);
            }
            // tool type 0..15: lets us mark a cancelled touch as a palm
            let mut s = UinputAbsSetup { code: ABS_MT_TOOL_TYPE, absinfo: AbsInfo { max: 15, ..AbsInfo::default() } };
            ioctl(fd, UI_SET_ABSBIT, ABS_MT_TOOL_TYPE as usize as *mut libc::c_void);
            ioctl(fd, UI_ABS_SETUP, &mut s);
            abss.insert(ABS_MT_TOOL_TYPE);
            continue;
        }
        let Some(&(_, nr)) = UI_SET_BIT.iter().find(|(ty, _)| *ty == t) else { continue };
        let codes: Vec<usize> = if t == EV_KEY {
            dev.keys.clone()
        } else {
            let len = 64;
            bits(dev.fd(), eviocgbit(t as u64, len as u64), len)
        };
        for c in codes {
            ioctl(fd, ioc(W, b'U', nr, 4), c as *mut libc::c_void);
            if t == EV_KEY {
                keys.insert(c as u16);
            }
        }
    }
    for &p in &dev.props {
        ioctl(fd, UI_SET_PROPBIT, p as *mut libc::c_void);
    }
    let mut id = InputId::default();
    ioctl(dev.fd(), EVIOCGID, &mut id);
    let mut setup = UinputSetup { id, name: [0; 80], ff_effects_max: 0 };
    setup.name[..NAME.len()].copy_from_slice(NAME.as_bytes());
    if ioctl(fd, UI_DEV_SETUP, &mut setup) < 0 || ioctl(fd, UI_DEV_CREATE, std::ptr::null_mut::<u8>()) < 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok((Uinput { file, buf: Vec::with_capacity(1024) }, abss, keys))
}

fn open_fifo() -> std::io::Result<File> {
    let _ = fs::remove_file(FIFO);
    let c = CString::new(FIFO).unwrap();
    if unsafe { libc::mkfifo(c.as_ptr(), 0o666) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    fs::set_permissions(FIFO, fs::Permissions::from_mode(0o666))?;
    OpenOptions::new().read(true).write(true).custom_flags(libc::O_NONBLOCK).open(FIFO)
}

fn read_fifo<O: Out>(fifo: &mut File, f: &mut Filter<O>) {
    let mut buf = [0u8; 4096];
    if let Ok(n) = fifo.read(&mut buf) {
        if let Some(&last) = buf[..n].iter().rev().find(|b| **b == b'0' || **b == b'1') {
            f.xournal = last == b'1';
            if f.test {
                println!("{}", if f.xournal { "Xournal++ actif" } else { "Xournal++ inactif" });
            }
        }
    }
}

fn monotonic() -> f64 {
    let mut t = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut t) };
    t.tv_sec as f64 + t.tv_nsec as f64 / 1e9
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() > 2 && args[1] == "--rejouer" {
        replay(&args[2]);
        return;
    }
    let test = args.iter().any(|a| a == "--test");
    let (screen, pen) = find();
    let Some(mut screen) = screen else {
        println!("Ecran tactile introuvable");
        std::process::exit(1);
    };
    let ax = screen.absinfo(ABS_MT_POSITION_X);
    let ay = screen.absinfo(ABS_MT_POSITION_Y);
    let geo = Geo::new((ax.min, ax.max, ax.res), (ay.min, ay.max, ay.res));
    println!("Ecran : {} ({:.0} x {:.0} mm)", screen.name, geo.w, geo.h);
    println!("Stylet : {}", pen.as_ref().map_or("aucun", |p| p.name.as_str()));

    let (out, abss, keys) = if test {
        println!("Mode test : touche ecran, Ctrl+C pour quitter");
        (None, HashSet::new(), HashSet::new())
    } else {
        match virtual_screen(&screen) {
            Ok((u, a, k)) => (Some(u), a, k),
            Err(e) => {
                println!("uinput : {e}");
                std::process::exit(1);
            }
        }
    };
    if !test && ioctl(screen.fd(), EVIOCGRAB, 1usize as *mut libc::c_void) < 0 {
        println!("grab impossible : {}", std::io::Error::last_os_error());
        std::process::exit(1);
    }
    let mut f = Filter::new(out, geo, keys, abss, test);
    let mut fifo = match open_fifo() {
        Ok(fd) => fd,
        Err(e) => {
            println!("fifo : {e}");
            std::process::exit(1);
        }
    };
    let mut pen = pen;
    let mut evs = Vec::with_capacity(64);
    loop {
        let mut fds = vec![
            libc::pollfd { fd: screen.fd(), events: libc::POLLIN, revents: 0 },
            libc::pollfd { fd: fifo.as_raw_fd(), events: libc::POLLIN, revents: 0 },
        ];
        if let Some(p) = &pen {
            fds.push(libc::pollfd { fd: p.fd(), events: libc::POLLIN, revents: 0 });
        }
        if unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, -1) } < 0 {
            continue;
        }
        if fds[1].revents & libc::POLLIN != 0 {
            read_fifo(&mut fifo, &mut f);
        }
        if fds[0].revents & libc::POLLIN != 0 {
            evs.clear();
            while screen.read_events(&mut evs) {}
            let now = monotonic();
            for e in &evs {
                f.screen(e.kind, e.code, e.value, now);
            }
        }
        if fds[0].revents & (libc::POLLERR | libc::POLLHUP) != 0 {
            println!("Ecran tactile perdu");
            std::process::exit(1); // systemd restarts us
        }
        if fds.get(2).is_some_and(|p| p.revents & (libc::POLLERR | libc::POLLHUP) != 0) {
            println!("Stylet perdu");
            pen = None; // otherwise poll() would wake up in a loop
        }
        if let (Some(p), Some(pfd)) = (pen.as_mut(), fds.get(2)) {
            if pfd.revents & libc::POLLIN != 0 {
                evs.clear();
                while p.read_events(&mut evs) {}
                let now = monotonic();
                for e in &evs {
                    f.pen(e.kind, e.code, e.value, now);
                }
            }
        }
    }
}

// replay: reads a recorded event file and prints what would be sent

struct Print;

impl Out for Print {
    fn emit(&mut self, t: u16, c: u16, v: i32) {
        println!("{t} {c} {v}");
    }
    fn syn(&mut self) {
        println!("SYN");
    }
}

// lines: "GEO xmin xmax xres ymin ymax yres", "KEYS c…", "ABS c…",
// "E t c v time" (screen), "P t c v time" (pen), "F 0|1" (Xournal++)
fn replay(path: &str) {
    let file = BufReader::new(File::open(path).expect("fichier"));
    let mut f: Option<Filter<Print>> = None;
    let (mut geo, mut keys, mut abss) = (None, HashSet::new(), HashSet::new());
    for line in file.lines().map_while(Result::ok) {
        let w: Vec<&str> = line.split_whitespace().collect();
        let n = |i: usize| w[i].parse::<f64>().unwrap();
        match w.first().copied() {
            Some("GEO") => geo = Some(Geo::new((n(1) as i32, n(2) as i32, n(3) as i32), (n(4) as i32, n(5) as i32, n(6) as i32))),
            Some("KEYS") => keys = w[1..].iter().map(|s| s.parse().unwrap()).collect(),
            Some("ABS") => abss = w[1..].iter().map(|s| s.parse().unwrap()).collect(),
            Some(kind) => {
                let filter = f.get_or_insert_with(|| Filter::new(Some(Print), geo.take().unwrap(), keys.clone(), abss.clone(), false));
                match kind {
                    "E" => filter.screen(n(1) as u16, n(2) as u16, n(3) as i32, n(4)),
                    "P" => filter.pen(n(1) as u16, n(2) as u16, n(3) as i32, n(4)),
                    "F" => filter.xournal = w[1] == "1",
                    _ => {}
                }
            }
            None => {}
        }
    }
}

// palm-guard-relay: KWin says whether Xournal++ is the active window; we pass
// it on to palm-guard (root) through its FIFO. Rust port of the Python relay:
// asleep until KWin calls SetActive.

use dbus::blocking::Connection;
use dbus::channel::MatchingReceiver;
use dbus::message::MatchRule;
use dbus_crossroads::Crossroads;
use std::fs::OpenOptions;
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::time::Duration;

const FIFO: &str = "/run/palm-guard.fifo";

fn send(on: bool) {
    // non-blocking: if palm-guard is not reading, we simply drop the message
    if let Ok(mut f) = OpenOptions::new().write(true).custom_flags(libc::O_NONBLOCK).open(FIFO) {
        let _ = f.write_all(if on { b"1\n" } else { b"0\n" });
    }
}

fn main() -> Result<(), dbus::Error> {
    let bus = Connection::new_session()?;
    bus.request_name("org.palmguard", false, true, false)?;
    let mut cr = Crossroads::new();
    let iface = cr.register("org.palmguard", |b| {
        b.method("SetActive", ("on",), (), |_, _: &mut (), (on,): (bool,)| {
            send(on);
            Ok(())
        });
    });
    cr.insert("/", &[iface], ());
    bus.start_receive(
        MatchRule::new_method_call(),
        Box::new(move |msg, conn| {
            let _ = cr.handle_message(msg, conn);
            true
        }),
    );
    loop {
        bus.process(Duration::from_secs(3600))?;
    }
}

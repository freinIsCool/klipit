use ashpd::desktop::screencast::Screencast;
use pipewire as pw;

use std::os::fd::OwnedFd;

pub fn connect(
    _session: ashpd::desktop::Session<Screencast>,
    fd: OwnedFd,
    node_id: u32,
) -> Result<(), Box<dyn std::error::Error>> {
    pw::init();

    let mainloop = pw::main_loop::MainLoopBox::new(None)?;
    let context = pw::context::ContextBox::new(&mainloop.loop_(), None)?;

    let core = context.connect_fd(fd, None)?;

    println!("Connected to PipeWire!");
    println!("Target node: {node_id}");

    let _ = core;
    Ok(())
}
use libspa::param::video::VideoFormat;
use pipewire::spa::utils::Direction;
use pipewire::{self as pw};

use pipewire::properties::properties;

use std::{
    cell::Cell,
    os::fd::OwnedFd,
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{Receiver, TryRecvError, TrySendError},
    },
    time::Duration,
};

const SEGMENT_DURATION_SECONDS: usize = 1;

pub fn connect(
    fd: OwnedFd,
    node_id: u32,
    shortcut_events: Receiver<()>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    pw::init();

    let mainloop = Rc::new(pw::main_loop::MainLoopBox::new(None)?);
    let context = pw::context::ContextBox::new(&mainloop.loop_(), None)?;

    let core = context.connect_fd(fd, None)?;

    let stream = pw::stream::StreamBox::new(
        &core,
        "record",
        properties! {
            *pw::keys::MEDIA_TYPE => "Video",
            *pw::keys::MEDIA_CATEGORY => "Capture",
            *pw::keys::MEDIA_ROLE => "Screen",
        },
    )?;

    let (frame_sender, recorder) =
        crate::recording::ffmpeg::start_segmented_recording(SEGMENT_DURATION_SECONDS)?;
    let audio_capture = crate::recording::audio::AudioCapture::start(frame_sender.clone())?;

    let ctrl_c_received = Arc::new(AtomicBool::new(false));
    let ctrl_c_flag = Arc::clone(&ctrl_c_received);
    ctrlc::set_handler(move || {
        ctrl_c_flag.store(true, Ordering::Release);
    })?;

    let state_mainloop = Rc::clone(&mainloop);
    let term_mainloop = Rc::clone(&mainloop);
    let _sigterm = mainloop
        .loop_()
        .add_signal_local(pw::loop_::Signal::SIGTERM, move || {
            term_mainloop.quit();
        });
    let timer_mainloop = Rc::clone(&mainloop);
    let timer_ctrl_c_flag = Arc::clone(&ctrl_c_received);
    let compile_sender = frame_sender.clone();
    let ctrl_c_timer = mainloop.loop_().add_timer({
        let save_requested = Cell::new(false);
        let shortcut_disconnected = Cell::new(false);
        move |_| {
            if timer_ctrl_c_flag.load(Ordering::Acquire) {
                eprintln!("Ctrl+C received; stopping capture and removing temporary segments.");
                timer_mainloop.quit();
                return;
            }

            if !save_requested.get() {
                match shortcut_events.try_recv() {
                    Ok(()) => save_requested.set(true),
                    Err(TryRecvError::Empty) => {}
                    Err(TryRecvError::Disconnected) if !shortcut_disconnected.get() => {
                        eprintln!("Global shortcut listener disconnected.");
                        shortcut_disconnected.set(true);
                    }
                    Err(TryRecvError::Disconnected) => {}
                }
            }

            if save_requested.get() {
                match compile_sender.try_send(crate::recording::ffmpeg::RecorderCommand::SaveClip) {
                    Ok(()) => save_requested.set(false),
                    Err(TrySendError::Full(_)) => {}
                    Err(TrySendError::Disconnected(_)) => {
                        eprintln!("Video recorder stopped; cannot save a clip.");
                        save_requested.set(false);
                    }
                }
            }
        }
    });
    let _listener = stream
        .add_local_listener_with_user_data(())
        .state_changed(move |_, _, old, new| {
            println!("STATE: {old:?} -> {new:?}");
            if old != pw::stream::StreamState::Unconnected
                && matches!(
                    new,
                    pw::stream::StreamState::Unconnected | pw::stream::StreamState::Error(_)
                )
            {
                state_mainloop.quit();
            }
        })
        .param_changed(|_, _, id, param| {
            print!("");
        })
        .add_buffer(|_, _, buffer| {
            print!("");
        })
        .process({
            let frame_sender = frame_sender.clone();
            let mut recorder_stopped = false;
            move |stream, _| {
                if let Some(mut buffer) = stream.dequeue_buffer() {
                    for data in buffer.datas_mut() {
                        if let Some(bytes) = data.data() {
                            if !recorder_stopped {
                                if let Err(err) = frame_sender.send(
                                    crate::recording::ffmpeg::RecorderCommand::Frame(
                                        bytes.to_vec(),
                                    ),
                                ) {
                                    eprintln!("Video segment writer stopped: {err}");
                                    recorder_stopped = true;
                                }
                            }
                        }
                    }
                }
            }
        })
        .register()?;

    use std::io::Cursor;

    use libspa::{
        param::{
            ParamType,
            format::{FormatProperties, MediaSubtype, MediaType},
        },
        pod::{Pod, Value, object, property, serialize::PodSerializer},
        utils::{Fraction, Rectangle, SpaTypes},
    };

    let object = Value::Object(object! {
        SpaTypes::ObjectParamFormat,
        ParamType::EnumFormat,

        property!(
            FormatProperties::MediaType,
            Id,
            MediaType::Video
        ),
        property!(
            FormatProperties::MediaSubtype,
            Id,
            MediaSubtype::Raw
        ),
        property!(
            FormatProperties::VideoFormat,
            Choice,
            Enum,
            Id,
            VideoFormat::BGRA,
            VideoFormat::BGRA,
            VideoFormat::BGRx,
            VideoFormat::RGBA,
            VideoFormat::RGBx,
            VideoFormat::RGB,
            VideoFormat::BGR,
            VideoFormat::ARGB,
            VideoFormat::xRGB,
            VideoFormat::ABGR,
            VideoFormat::xBGR,
            VideoFormat::YUY2,
            VideoFormat::NV12,
            VideoFormat::I420
        ),
        property!(
            FormatProperties::VideoSize,
            Choice,
            Range,
            Rectangle,
            Rectangle {
                width: 1920,
                height: 1080,
            },
            Rectangle {
                width: 1,
                height: 1,
            },
            Rectangle {
                width: 8192,
                height: 8192,
            }
        ),
        property!(
            FormatProperties::VideoFramerate,
            Choice,
            Range,
            Fraction,
            Fraction {
                num: 60,
                denom: 1,
            },
            Fraction {
                num: 0,
                denom: 1,
            },
            Fraction {
                num: 1000,
                denom: 1,
            }
        ),
    });

    let bytes = PodSerializer::serialize(Cursor::new(Vec::new()), &object)?
        .0
        .into_inner();

    let pod = Pod::from_bytes(&bytes).ok_or("Failed to create SPA Pod")?;

    let mut params = [&*pod];

    stream.connect(
        Direction::Input,
        Some(node_id),
        pw::stream::StreamFlags::AUTOCONNECT | pw::stream::StreamFlags::MAP_BUFFERS,
        &mut params,
    )?;

    println!("{:?}", stream.node_id());
    println!("Connected to PipeWire!");
    println!("Target node: {node_id}");

    ctrl_c_timer
        .update_timer(
            Some(Duration::from_millis(100)),
            Some(Duration::from_millis(100)),
        )
        .into_result()?;
    mainloop.run();
    drop(_listener);
    drop(_sigterm);
    drop(ctrl_c_timer);
    eprintln!("Capture stopped; finalizing segment files and cleaning up temporary segments.");
    let audio_result = audio_capture.stop();
    drop(frame_sender);
    let recorder_result = recorder
        .join()
        .map_err(|_| std::io::Error::other("Video segment writer thread panicked"))?;
    audio_result.map_err(std::io::Error::other)?;
    recorder_result.map_err(std::io::Error::other)?;

    Ok(())
}

use libspa::param::video::VideoFormat;
use pipewire::spa::utils::Direction;
use pipewire::{self as pw};

use pipewire::properties::properties;

use std::{os::fd::OwnedFd, rc::Rc};

const SEGMENT_DURATION_SECONDS: usize = 2;

pub fn connect(fd: OwnedFd, node_id: u32) -> Result<(), Box<dyn std::error::Error>> {
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

    let state_mainloop = Rc::clone(&mainloop);
    let signal_mainloop = Rc::clone(&mainloop);
    let _sigint = mainloop
        .loop_()
        .add_signal_local(pw::loop_::Signal::SIGINT, move || {
            signal_mainloop.quit();
        });
    let term_mainloop = Rc::clone(&mainloop);
    let _sigterm = mainloop
        .loop_()
        .add_signal_local(pw::loop_::Signal::SIGTERM, move || {
            term_mainloop.quit();
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
            println!("PARAM: id={id}, param={:?}", param.map(|p| p.as_bytes()));
        })
        .add_buffer(|_, _, buffer| {
            println!("BUFFER ADDED: {buffer:?}");
        })
        .process({
            let mut recorder_stopped = false;
            move |stream, _| {
                if let Some(mut buffer) = stream.dequeue_buffer() {
                    for data in buffer.datas_mut() {
                        if let Some(bytes) = data.data() {
                            if !recorder_stopped {
                                if let Err(err) = frame_sender.send(bytes.to_vec()) {
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

    mainloop.run();
    drop(_listener);
    drop(_sigint);
    drop(_sigterm);
    recorder
        .join()
        .map_err(|_| std::io::Error::other("Video segment writer thread panicked"))?
        .map_err(std::io::Error::other)?;

    Ok(())
}

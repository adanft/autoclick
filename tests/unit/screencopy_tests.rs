    use opencv::core::CV_8UC1;
    use opencv::prelude::MatTraitConst;

    /// BT.601 luma OpenCV produces for saturated primaries and white.
    const RED_GRAY: u8 = 76;
    const GREEN_GRAY: u8 = 150;
    const BLUE_GRAY: u8 = 29;
    const WHITE_GRAY: u8 = 255;

    fn layout(format: Format, width: u32, height: u32, stride: u32) -> FrameLayout {
        FrameLayout {
            format,
            width,
            height,
            stride,
        }
    }

    /// Lays out rows of 4-byte pixels with `padding` trailing bytes per row.
    fn frame_bytes(rows: &[&[[u8; 4]]], padding: usize) -> Vec<u8> {
        rows.iter()
            .flat_map(|row| {
                row.iter()
                    .flatten()
                    .copied()
                    .chain(std::iter::repeat_n(0xAB, padding))
            })
            .collect()
    }

    fn gray_rows(mat: &Mat) -> Vec<Vec<u8>> {
        (0..mat.rows())
            .map(|row| {
                (0..mat.cols())
                    .map(|col| *mat.at_2d::<u8>(row, col).unwrap())
                    .collect()
            })
            .collect()
    }

    // Memory order on little-endian: Xrgb8888/Argb8888 are B,G,R,X and
    // Xbgr8888/Abgr8888 are R,G,B,X.
    const BGRX_RED: [u8; 4] = [0, 0, 255, 0];
    const BGRX_GREEN: [u8; 4] = [0, 255, 0, 0];
    const BGRX_BLUE: [u8; 4] = [255, 0, 0, 0];
    const WHITE: [u8; 4] = [255, 255, 255, 255];

    #[test]
    fn converts_xrgb_frames_as_bgra_memory_order() {
        let pixels = frame_bytes(&[&[BGRX_RED, BGRX_GREEN], &[BGRX_BLUE, WHITE]], 0);

        for format in [Format::Xrgb8888, Format::Argb8888] {
            let gray = shm_frame_to_grayscale(&pixels, &layout(format, 2, 2, 8), false).unwrap();

            assert_eq!(gray.typ(), CV_8UC1);
            assert_eq!(
                gray_rows(&gray),
                vec![vec![RED_GRAY, GREEN_GRAY], vec![BLUE_GRAY, WHITE_GRAY]]
            );
        }
    }

    #[test]
    fn converts_xbgr_frames_as_rgba_memory_order() {
        // The same bytes read as R,G,B,X swap red and blue.
        let pixels = frame_bytes(&[&[BGRX_RED, BGRX_GREEN], &[BGRX_BLUE, WHITE]], 0);

        for format in [Format::Xbgr8888, Format::Abgr8888] {
            let gray = shm_frame_to_grayscale(&pixels, &layout(format, 2, 2, 8), false).unwrap();

            assert_eq!(
                gray_rows(&gray),
                vec![vec![BLUE_GRAY, GREEN_GRAY], vec![RED_GRAY, WHITE_GRAY]]
            );
        }
    }

    #[test]
    fn skips_row_padding_beyond_the_visible_width() {
        let pixels = frame_bytes(
            &[&[BGRX_RED, BGRX_GREEN, BGRX_BLUE], &[WHITE, WHITE, BGRX_RED]],
            4,
        );

        let gray =
            shm_frame_to_grayscale(&pixels, &layout(Format::Xrgb8888, 3, 2, 16), false).unwrap();

        assert_eq!((gray.cols(), gray.rows()), (3, 2));
        assert_eq!(
            gray_rows(&gray),
            vec![
                vec![RED_GRAY, GREEN_GRAY, BLUE_GRAY],
                vec![WHITE_GRAY, WHITE_GRAY, RED_GRAY]
            ]
        );
    }

    #[test]
    fn flips_vertically_when_the_frame_is_y_inverted() {
        let pixels = frame_bytes(&[&[BGRX_RED, BGRX_GREEN], &[BGRX_BLUE, WHITE]], 4);

        let gray =
            shm_frame_to_grayscale(&pixels, &layout(Format::Xrgb8888, 2, 2, 12), true).unwrap();

        assert_eq!(
            gray_rows(&gray),
            vec![vec![BLUE_GRAY, WHITE_GRAY], vec![RED_GRAY, GREEN_GRAY]]
        );
    }

    #[test]
    fn rejects_unsupported_formats() {
        let pixels = vec![0; 8];

        let error = shm_frame_to_grayscale(&pixels, &layout(Format::Rgb565, 2, 1, 8), false)
            .unwrap_err();

        assert!(
            error.to_string().contains("unsupported wl_shm format"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn rejects_empty_or_oversized_dimensions() {
        for (width, height, stride) in [(0, 2, 8), (2, 0, 8), (u32::MAX, 1, u32::MAX)] {
            let error = shm_frame_to_grayscale(
                &[],
                &layout(Format::Xrgb8888, width, height, stride),
                false,
            )
            .unwrap_err();

            assert!(
                error.to_string().contains("frame extent"),
                "unexpected error for {width}x{height}: {error}"
            );
        }
    }

    #[test]
    fn rejects_strides_that_cannot_hold_a_row_of_whole_pixels() {
        for stride in [4, 10] {
            let pixels = vec![0; stride as usize * 2];

            let error =
                shm_frame_to_grayscale(&pixels, &layout(Format::Xrgb8888, 2, 2, stride), false)
                    .unwrap_err();

            assert!(
                error.to_string().contains("stride"),
                "unexpected error for stride {stride}: {error}"
            );
        }
    }

    #[test]
    fn rejects_pixel_data_that_does_not_match_stride_times_height() {
        for length in [15, 17] {
            let pixels = vec![0; length];

            let error = shm_frame_to_grayscale(&pixels, &layout(Format::Xrgb8888, 2, 2, 8), false)
                .unwrap_err();

            assert!(
                error.to_string().contains("expected 16 bytes"),
                "unexpected error for {length} bytes: {error}"
            );
        }
    }

    /// In-process stand-in for a wlroots compositor: advertises screencopy,
    /// `wl_shm` and named outputs, and answers each `copy` from a script.
    mod fake {
        use std::collections::VecDeque;
        use std::fs::File;
        use std::os::unix::{fs::FileExt, net::UnixStream};
        use std::sync::{mpsc, Arc};
        use std::thread::{self, JoinHandle};
        use wayland_client::Connection;
        use wayland_protocols_wlr::screencopy::v1::server::{
            zwlr_screencopy_frame_v1::{self, ZwlrScreencopyFrameV1},
            zwlr_screencopy_manager_v1::{self, ZwlrScreencopyManagerV1},
        };
        use wayland_server::protocol::{
            wl_buffer::{self, WlBuffer},
            wl_output::{self, WlOutput},
            wl_shm::{self, WlShm},
            wl_shm_pool::{self, WlShmPool},
        };
        use wayland_server::{
            Client, DataInit, Dispatch, Display, DisplayHandle, GlobalDispatch, New, Resource,
        };

        pub enum Reply {
            Ready { y_invert: bool },
            Failed,
        }

        pub struct Scenario {
            pub advertise_manager: bool,
            pub outputs: Vec<&'static str>,
            pub width: u32,
            pub height: u32,
            pub stride: u32,
            /// `stride * height` bytes written into the client buffer on `Ready`.
            pub pixels: Vec<u8>,
            pub replies: Vec<Reply>,
        }

        #[derive(Debug, Default, PartialEq, Eq)]
        pub struct Report {
            pub captured_outputs: Vec<String>,
            pub overlay_cursors: Vec<i32>,
            pub pools: usize,
            pub buffers: usize,
            pub copies: usize,
            pub frames_destroyed: usize,
        }

        struct ServerState {
            width: u32,
            height: u32,
            stride: u32,
            pixels: Vec<u8>,
            replies: VecDeque<Reply>,
            report: Report,
        }

        struct BufferData {
            file: Arc<File>,
            offset: i32,
        }

        impl GlobalDispatch<WlOutput, &'static str> for ServerState {
            fn bind(
                _: &mut Self,
                _: &DisplayHandle,
                _: &Client,
                resource: New<WlOutput>,
                name: &&'static str,
                data_init: &mut DataInit<'_, Self>,
            ) {
                let output = data_init.init(resource, *name);
                output.name((*name).into());
                output.done();
            }
        }
        #[rustfmt::skip]
        impl Dispatch<WlOutput, &'static str> for ServerState {
            fn request(_: &mut Self, _: &Client, _: &WlOutput, _: wl_output::Request, _: &&'static str, _: &DisplayHandle, _: &mut DataInit<'_, Self>) {}
        }

        impl GlobalDispatch<WlShm, ()> for ServerState {
            fn bind(
                _: &mut Self,
                _: &DisplayHandle,
                _: &Client,
                resource: New<WlShm>,
                _: &(),
                data_init: &mut DataInit<'_, Self>,
            ) {
                let shm = data_init.init(resource, ());
                shm.format(wl_shm::Format::Argb8888);
                shm.format(wl_shm::Format::Xrgb8888);
            }
        }
        impl Dispatch<WlShm, ()> for ServerState {
            fn request(
                state: &mut Self,
                _: &Client,
                _: &WlShm,
                request: wl_shm::Request,
                _: &(),
                _: &DisplayHandle,
                data_init: &mut DataInit<'_, Self>,
            ) {
                if let wl_shm::Request::CreatePool { id, fd, .. } = request {
                    state.report.pools += 1;
                    data_init.init(id, Arc::new(File::from(fd)));
                }
            }
        }
        impl Dispatch<WlShmPool, Arc<File>> for ServerState {
            fn request(
                state: &mut Self,
                _: &Client,
                _: &WlShmPool,
                request: wl_shm_pool::Request,
                file: &Arc<File>,
                _: &DisplayHandle,
                data_init: &mut DataInit<'_, Self>,
            ) {
                if let wl_shm_pool::Request::CreateBuffer { id, offset, .. } = request {
                    state.report.buffers += 1;
                    data_init.init(
                        id,
                        BufferData {
                            file: file.clone(),
                            offset,
                        },
                    );
                }
            }
        }
        #[rustfmt::skip]
        impl Dispatch<WlBuffer, BufferData> for ServerState {
            fn request(_: &mut Self, _: &Client, _: &WlBuffer, _: wl_buffer::Request, _: &BufferData, _: &DisplayHandle, _: &mut DataInit<'_, Self>) {}
        }

        impl GlobalDispatch<ZwlrScreencopyManagerV1, ()> for ServerState {
            fn bind(
                _: &mut Self,
                _: &DisplayHandle,
                _: &Client,
                resource: New<ZwlrScreencopyManagerV1>,
                _: &(),
                data_init: &mut DataInit<'_, Self>,
            ) {
                data_init.init(resource, ());
            }
        }
        impl Dispatch<ZwlrScreencopyManagerV1, ()> for ServerState {
            fn request(
                state: &mut Self,
                _: &Client,
                _: &ZwlrScreencopyManagerV1,
                request: zwlr_screencopy_manager_v1::Request,
                _: &(),
                _: &DisplayHandle,
                data_init: &mut DataInit<'_, Self>,
            ) {
                if let zwlr_screencopy_manager_v1::Request::CaptureOutput {
                    frame,
                    overlay_cursor,
                    output,
                } = request
                {
                    let name = output.data::<&'static str>().copied().unwrap_or("?");
                    state.report.captured_outputs.push(name.into());
                    state.report.overlay_cursors.push(overlay_cursor);
                    let frame = data_init.init(frame, ());
                    // A dmabuf offer first: the client must skip it for the shm one.
                    frame.linux_dmabuf(0x3432_5258, state.width, state.height);
                    frame.buffer(
                        wl_shm::Format::Xrgb8888,
                        state.width,
                        state.height,
                        state.stride,
                    );
                    frame.buffer_done();
                }
            }
        }
        impl Dispatch<ZwlrScreencopyFrameV1, ()> for ServerState {
            fn request(
                state: &mut Self,
                _: &Client,
                frame: &ZwlrScreencopyFrameV1,
                request: zwlr_screencopy_frame_v1::Request,
                _: &(),
                _: &DisplayHandle,
                _: &mut DataInit<'_, Self>,
            ) {
                match request {
                    zwlr_screencopy_frame_v1::Request::Copy { buffer } => {
                        state.report.copies += 1;
                        match state.replies.pop_front() {
                            Some(Reply::Ready { y_invert }) => {
                                let data = buffer.data::<BufferData>().unwrap();
                                data.file
                                    .write_all_at(&state.pixels, data.offset as u64)
                                    .unwrap();
                                frame.flags(if y_invert {
                                    zwlr_screencopy_frame_v1::Flags::YInvert
                                } else {
                                    zwlr_screencopy_frame_v1::Flags::empty()
                                });
                                frame.ready(0, 0, 0);
                            }
                            Some(Reply::Failed) | None => frame.failed(),
                        }
                    }
                    zwlr_screencopy_frame_v1::Request::Destroy => {
                        state.report.frames_destroyed += 1
                    }
                    _ => {}
                }
            }
        }

        pub struct FakeCompositor {
            stop: mpsc::Sender<()>,
            thread: JoinHandle<Report>,
        }

        impl FakeCompositor {
            pub fn spawn(scenario: Scenario) -> (Self, Connection) {
                let (client, server) = UnixStream::pair().unwrap();
                let (stop, stopped) = mpsc::channel();
                let thread = thread::spawn(move || {
                    let mut display = Display::<ServerState>::new().unwrap();
                    let mut handle = display.handle();
                    handle.create_global::<ServerState, WlShm, _>(1, ());
                    for name in scenario.outputs {
                        handle.create_global::<ServerState, WlOutput, _>(4, name);
                    }
                    if scenario.advertise_manager {
                        handle.create_global::<ServerState, ZwlrScreencopyManagerV1, _>(3, ());
                    }
                    server.set_nonblocking(true).unwrap();
                    handle.insert_client(server, Arc::new(())).unwrap();
                    let mut state = ServerState {
                        width: scenario.width,
                        height: scenario.height,
                        stride: scenario.stride,
                        pixels: scenario.pixels,
                        replies: scenario.replies.into(),
                        report: Report::default(),
                    };
                    while let Err(mpsc::TryRecvError::Empty) = stopped.try_recv() {
                        if display.dispatch_clients(&mut state).is_ok() {
                            display.flush_clients().unwrap();
                        } else {
                            thread::yield_now();
                        }
                    }
                    state.report
                });
                (Self { stop, thread }, Connection::from_socket(client).unwrap())
            }

            pub fn finish(self) -> Report {
                self.stop.send(()).unwrap();
                self.thread.join().unwrap()
            }
        }
    }

    use fake::{FakeCompositor, Reply, Report, Scenario};

    /// A 3x2 Xrgb8888 frame with four bytes of row padding.
    fn padded_scenario(replies: Vec<Reply>) -> Scenario {
        Scenario {
            advertise_manager: true,
            outputs: vec!["DP-1", "HDMI-A-1"],
            width: 3,
            height: 2,
            stride: 16,
            pixels: frame_bytes(
                &[&[BGRX_RED, BGRX_GREEN, BGRX_BLUE], &[WHITE, WHITE, BGRX_RED]],
                4,
            ),
            replies,
        }
    }

    const TOP_ROW: [u8; 3] = [RED_GRAY, GREEN_GRAY, BLUE_GRAY];
    const BOTTOM_ROW: [u8; 3] = [WHITE_GRAY, WHITE_GRAY, RED_GRAY];

    /// Waits until the fake compositor has handled every request sent so far.
    fn settle(client: &mut ScreencopyClient) {
        client.event_queue.roundtrip(&mut client.state).unwrap();
    }

    #[test]
    fn captures_the_named_output_into_a_grayscale_mat() {
        let (compositor, connection) =
            FakeCompositor::spawn(padded_scenario(vec![Reply::Ready { y_invert: false }]));
        let mut client = ScreencopyClient::from_connection(connection, "HDMI-A-1").unwrap();

        let gray = client.capture().unwrap();
        settle(&mut client);

        assert_eq!(gray_rows(&gray), vec![TOP_ROW.to_vec(), BOTTOM_ROW.to_vec()]);
        assert_eq!(
            compositor.finish(),
            Report {
                captured_outputs: vec!["HDMI-A-1".into()],
                overlay_cursors: vec![0],
                pools: 1,
                buffers: 1,
                copies: 1,
                frames_destroyed: 1,
            }
        );
    }

    #[test]
    fn reuses_one_shm_buffer_across_captures_of_the_same_layout() {
        let (compositor, connection) = FakeCompositor::spawn(padded_scenario(vec![
            Reply::Ready { y_invert: false },
            Reply::Ready { y_invert: true },
        ]));
        let mut client = ScreencopyClient::from_connection(connection, "HDMI-A-1").unwrap();

        let first = client.capture().unwrap();
        let second = client.capture().unwrap();
        settle(&mut client);

        assert_eq!(gray_rows(&first), vec![TOP_ROW.to_vec(), BOTTOM_ROW.to_vec()]);
        assert_eq!(gray_rows(&second), vec![BOTTOM_ROW.to_vec(), TOP_ROW.to_vec()]);
        let report = compositor.finish();
        assert_eq!((report.pools, report.buffers), (1, 1));
        assert_eq!((report.copies, report.frames_destroyed), (2, 2));
    }

    #[test]
    fn a_failed_frame_does_not_poison_the_next_capture() {
        let (compositor, connection) = FakeCompositor::spawn(padded_scenario(vec![
            Reply::Failed,
            Reply::Ready { y_invert: false },
        ]));
        let mut client = ScreencopyClient::from_connection(connection, "HDMI-A-1").unwrap();

        let error = client.capture().unwrap_err();
        let recovered = client.capture().unwrap();
        settle(&mut client);

        assert!(
            error.to_string().contains("screencopy frame as failed"),
            "unexpected error: {error}"
        );
        assert_eq!(
            gray_rows(&recovered),
            vec![TOP_ROW.to_vec(), BOTTOM_ROW.to_vec()]
        );
        let report = compositor.finish();
        assert_eq!((report.pools, report.copies, report.frames_destroyed), (1, 2, 2));
    }

    #[test]
    fn connect_fails_when_the_compositor_lacks_screencopy() {
        let (compositor, connection) = FakeCompositor::spawn(Scenario {
            advertise_manager: false,
            ..padded_scenario(Vec::new())
        });

        let error = ScreencopyClient::from_connection(connection, "HDMI-A-1")
            .err()
            .unwrap();

        assert!(
            error.to_string().contains("zwlr_screencopy_manager_v1"),
            "unexpected error: {error}"
        );
        compositor.finish();
    }

    #[test]
    fn connect_fails_for_an_unknown_connector() {
        let (compositor, connection) = FakeCompositor::spawn(padded_scenario(Vec::new()));

        let error = ScreencopyClient::from_connection(connection, "DP-9")
            .err()
            .unwrap();

        assert!(
            error
                .to_string()
                .contains("configured connector DP-9 was not found"),
            "unexpected error: {error}"
        );
        compositor.finish();
    }

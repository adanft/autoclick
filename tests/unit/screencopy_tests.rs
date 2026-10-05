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

    #[test]
    fn samples_region_colors_top_down_for_every_channel_order() {
        const BLACK: [u8; 4] = [0, 0, 0, 0];
        const GRAY_100: [u8; 4] = [100, 100, 100, 0];
        let bgrx = [[BGRX_RED, BGRX_GREEN, BGRX_BLUE], [WHITE, BLACK, GRAY_100]];
        let rgbx = bgrx.map(|row| row.map(|[b, g, r, x]| [r, g, b, x]));
        let right_of_top_row = Rect::new(1, 0, 2, 1);

        for (format, rows) in [
            (Format::Xrgb8888, bgrx),
            (Format::Argb8888, bgrx),
            (Format::Xbgr8888, rgbx),
            (Format::Abgr8888, rgbx),
        ] {
            // Three pixels per row plus four padding bytes.
            let pixels = frame_bytes(&[&rows[0], &rows[1]], 4);
            let frame = layout(format, 3, 2, 16);

            let upright = shm_region_stats(&pixels, &frame, false, right_of_top_row).unwrap();
            let inverted = shm_region_stats(&pixels, &frame, true, right_of_top_row).unwrap();

            // Green and blue: luma 150 and 29.
            assert_eq!(upright.mean_bgr, [127.5, 127.5, 0.0], "{format:?}");
            assert_eq!(upright.luma_std, 60.5, "{format:?}");
            // A y-inverted frame stores the top row last: black and gray 100.
            assert_eq!(inverted.mean_bgr, [50.0; 3], "{format:?}");
            assert_eq!(inverted.luma_std, 50.0, "{format:?}");
        }
    }

    #[test]
    fn region_luma_spread_equals_the_grayscale_frame() {
        let pixels = frame_bytes(&[&[BGRX_RED, BGRX_GREEN], &[BGRX_BLUE, WHITE]], 4);
        let frame = layout(Format::Xrgb8888, 2, 2, 12);
        let gray = shm_frame_to_grayscale(&pixels, &frame, true).unwrap();

        let stats = shm_region_stats(&pixels, &frame, true, Rect::new(0, 0, 2, 2)).unwrap();

        let mut mean = Mat::default();
        let mut stddev = Mat::default();
        core::mean_std_dev(&gray, &mut mean, &mut stddev, &core::no_array()).unwrap();
        assert_eq!(stats.luma_std, *stddev.at_2d::<f64>(0, 0).unwrap());
        assert_eq!(stats.mean_bgr, [127.5, 127.5, 127.5]);
    }

    #[test]
    fn rejects_regions_outside_the_frame_or_without_pixels() {
        let pixels = frame_bytes(&[&[BGRX_RED, BGRX_GREEN], &[BGRX_BLUE, WHITE]], 0);
        let frame = layout(Format::Xrgb8888, 2, 2, 8);

        for region in [
            Rect::new(1, 1, 2, 1),
            Rect::new(-1, 0, 1, 1),
            Rect::new(0, 0, 0, 1),
            Rect::new(i32::MAX, 0, 1, 1),
        ] {
            let error = shm_region_stats(&pixels, &frame, false, region).unwrap_err();

            assert!(
                error.to_string().contains("outside the 2x2 frame"),
                "unexpected error for {region:?}: {error}"
            );
        }
        assert!(shm_region_stats(&pixels[1..], &frame, false, Rect::new(0, 0, 1, 1)).is_err());
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
        use wayland_server::backend::GlobalId;
        use wayland_server::{
            Client, DataInit, Dispatch, Display, DisplayHandle, GlobalDispatch, New, Resource,
        };

        /// The compositor-side `wl_shm` format, distinct from the client one.
        pub use wayland_server::protocol::wl_shm::Format as ServerFormat;

        #[derive(Clone)]
        pub enum Reply {
            Ready { y_invert: bool },
            Failed,
            /// Never answers the copy, like a stalled compositor.
            Silent,
        }

        pub struct Scenario {
            pub advertise_manager: bool,
            /// Advertised manager version; below 3 there is no `buffer_done`
            /// and no dmabuf offer.
            pub manager_version: u32,
            /// `wl_shm` formats offered per captured frame, in order; once
            /// exhausted every frame offers `Xrgb8888` alone.
            pub offers: Vec<Vec<ServerFormat>>,
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
            offers: VecDeque<Vec<ServerFormat>>,
            replies: VecDeque<Reply>,
            report: Report,
        }

        enum Command {
            Stop,
            /// Removes the named output's global, then acknowledges once the
            /// `global_remove` event is flushed to the client.
            RemoveOutput(&'static str, mpsc::Sender<()>),
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
                    let has_buffer_done = frame.version() >= 3;
                    if has_buffer_done {
                        // A dmabuf offer first: the client must skip it for the shm one.
                        frame.linux_dmabuf(0x3432_5258, state.width, state.height);
                    }
                    let formats = state
                        .offers
                        .pop_front()
                        .unwrap_or_else(|| vec![ServerFormat::Xrgb8888]);
                    for format in formats {
                        frame.buffer(format, state.width, state.height, state.stride);
                    }
                    if has_buffer_done {
                        frame.buffer_done();
                    }
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
                            Some(Reply::Silent) => {}
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
            commands: mpsc::Sender<Command>,
            thread: JoinHandle<Report>,
        }

        impl FakeCompositor {
            pub fn spawn(scenario: Scenario) -> (Self, Connection) {
                let (client, server) = UnixStream::pair().unwrap();
                let (commands, received) = mpsc::channel();
                let thread = thread::spawn(move || {
                    let mut display = Display::<ServerState>::new().unwrap();
                    let mut handle = display.handle();
                    handle.create_global::<ServerState, WlShm, _>(1, ());
                    let outputs: Vec<(&'static str, GlobalId)> = scenario
                        .outputs
                        .into_iter()
                        .map(|name| {
                            (name, handle.create_global::<ServerState, WlOutput, _>(4, name))
                        })
                        .collect();
                    if scenario.advertise_manager {
                        handle.create_global::<ServerState, ZwlrScreencopyManagerV1, _>(
                            scenario.manager_version,
                            (),
                        );
                    }
                    server.set_nonblocking(true).unwrap();
                    handle.insert_client(server, Arc::new(())).unwrap();
                    let mut state = ServerState {
                        width: scenario.width,
                        height: scenario.height,
                        stride: scenario.stride,
                        pixels: scenario.pixels,
                        offers: scenario.offers.into(),
                        replies: scenario.replies.into(),
                        report: Report::default(),
                    };
                    loop {
                        match received.try_recv() {
                            Ok(Command::Stop) | Err(mpsc::TryRecvError::Disconnected) => break,
                            Ok(Command::RemoveOutput(name, ack)) => {
                                let (_, id) = outputs.iter().find(|(n, _)| *n == name).unwrap();
                                handle.remove_global::<ServerState>(id.clone());
                                display.flush_clients().unwrap();
                                ack.send(()).unwrap();
                            }
                            Err(mpsc::TryRecvError::Empty) => {}
                        }
                        if display.dispatch_clients(&mut state).is_ok() {
                            display.flush_clients().unwrap();
                        } else {
                            thread::yield_now();
                        }
                    }
                    state.report
                });
                (Self { commands, thread }, Connection::from_socket(client).unwrap())
            }

            /// Removes the named output's global and returns once the client
            /// has been sent `global_remove`; a later roundtrip delivers it.
            pub fn remove_output(&self, name: &'static str) {
                let (ack, acked) = mpsc::channel();
                self.commands
                    .send(Command::RemoveOutput(name, ack))
                    .unwrap();
                acked.recv().unwrap();
            }

            pub fn finish(self) -> Report {
                self.commands.send(Command::Stop).unwrap();
                self.thread.join().unwrap()
            }
        }
    }

    use fake::{FakeCompositor, Reply, Report, Scenario, ServerFormat};

    /// A 3x2 Xrgb8888 frame with four bytes of row padding.
    fn padded_scenario(replies: Vec<Reply>) -> Scenario {
        Scenario {
            advertise_manager: true,
            manager_version: 3,
            offers: Vec::new(),
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
    fn samples_regions_of_the_last_captured_frame_only() {
        let (compositor, connection) = FakeCompositor::spawn(padded_scenario(vec![
            Reply::Ready { y_invert: true },
            Reply::Failed,
        ]));
        let mut client = ScreencopyClient::from_connection(connection, "HDMI-A-1").unwrap();
        let top_left = Rect::new(0, 0, 2, 1);

        let before = client.region_stats(top_left).unwrap_err();
        client.capture().unwrap();
        // The frame is y-inverted, so its top-left pixels are the two whites.
        let sampled = client.region_stats(top_left).unwrap();
        client.capture().unwrap_err();
        let after_failure = client.region_stats(top_left).unwrap_err();
        settle(&mut client);

        assert_eq!(sampled.mean_bgr, [255.0; 3]);
        assert_eq!(sampled.luma_std, 0.0);
        for error in [before, after_failure] {
            assert!(
                error.to_string().contains("no captured frame"),
                "unexpected error: {error}"
            );
        }
        compositor.finish();
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
    fn a_frame_that_never_completes_times_out_and_the_next_capture_succeeds() {
        let (compositor, connection) = FakeCompositor::spawn(padded_scenario(vec![
            Reply::Silent,
            Reply::Ready { y_invert: false },
        ]));
        let mut client = ScreencopyClient::from_connection(connection, "HDMI-A-1").unwrap();
        client.frame_timeout = std::time::Duration::from_millis(100);

        let started = std::time::Instant::now();
        let error = client.capture().unwrap_err();
        let waited = started.elapsed();
        let recovered = client.capture().unwrap();
        settle(&mut client);

        assert_eq!(
            error.to_string(),
            "screencopy frame for output HDMI-A-1 did not complete within 100 ms"
        );
        assert!(
            waited >= std::time::Duration::from_millis(100)
                && waited < std::time::Duration::from_secs(2),
            "timed out after {waited:?}"
        );
        assert_eq!(
            gray_rows(&recovered),
            vec![TOP_ROW.to_vec(), BOTTOM_ROW.to_vec()]
        );
        let report = compositor.finish();
        assert_eq!((report.pools, report.copies, report.frames_destroyed), (1, 2, 2));
    }

    #[test]
    fn a_pre_v3_manager_captures_from_its_single_buffer_offer() {
        // Before v3 there is no `buffer_done`: the lone `buffer` event closes
        // the offer, so waiting for `buffer_done` would stall until the timeout.
        for manager_version in [1, 2] {
            let (compositor, connection) = FakeCompositor::spawn(Scenario {
                manager_version,
                ..padded_scenario(vec![Reply::Ready { y_invert: false }])
            });
            let mut client = ScreencopyClient::from_connection(connection, "HDMI-A-1").unwrap();
            client.frame_timeout = std::time::Duration::from_millis(500);

            let gray = client.capture().unwrap();
            settle(&mut client);

            assert_eq!(client.manager.version(), manager_version);
            assert_eq!(gray_rows(&gray), vec![TOP_ROW.to_vec(), BOTTOM_ROW.to_vec()]);
            let report = compositor.finish();
            assert_eq!((report.pools, report.copies, report.frames_destroyed), (1, 1, 1));
        }
    }

    #[test]
    fn a_frame_offering_only_unsupported_formats_fails_fast_and_the_next_succeeds() {
        // A v3 frame may offer several formats before `buffer_done`; a pre-v3
        // frame offers exactly one.
        let cases = [
            (3, vec![ServerFormat::Rgb565, ServerFormat::Rgb888]),
            (1, vec![ServerFormat::Rgb565]),
        ];
        for (manager_version, unsupported) in cases {
            let (compositor, connection) = FakeCompositor::spawn(Scenario {
                manager_version,
                offers: vec![unsupported],
                ..padded_scenario(vec![Reply::Ready { y_invert: false }])
            });
            let mut client = ScreencopyClient::from_connection(connection, "HDMI-A-1").unwrap();

            let started = std::time::Instant::now();
            let error = client.capture().unwrap_err();
            let waited = started.elapsed();
            let recovered = client.capture().unwrap();
            settle(&mut client);

            let message = error.to_string();
            assert!(
                message.starts_with("the compositor offered no supported wl_shm screencopy format")
                    && message.contains("Rgb565"),
                "unexpected error for v{manager_version}: {message}"
            );
            assert!(
                waited < client.frame_timeout / 4,
                "v{manager_version} failed after {waited:?}"
            );
            assert_eq!(
                gray_rows(&recovered),
                vec![TOP_ROW.to_vec(), BOTTOM_ROW.to_vec()]
            );
            // The rejected frame is destroyed without ever being copied.
            let report = compositor.finish();
            assert_eq!((report.pools, report.copies, report.frames_destroyed), (1, 1, 2));
        }
    }

    #[test]
    fn removing_the_selected_output_fails_every_later_capture_fast() {
        let (compositor, connection) =
            FakeCompositor::spawn(padded_scenario(vec![Reply::Ready { y_invert: false }; 3]));
        let mut client = ScreencopyClient::from_connection(connection, "HDMI-A-1").unwrap();
        client.capture().unwrap();

        compositor.remove_output("HDMI-A-1");
        settle(&mut client);

        for attempt in 1..=2 {
            let started = std::time::Instant::now();
            let error = client.capture().unwrap_err();
            let waited = started.elapsed();

            assert_eq!(error.to_string(), "Wayland output HDMI-A-1 was removed");
            assert!(
                waited < client.frame_timeout / 4,
                "attempt {attempt} failed after {waited:?}"
            );
        }
        settle(&mut client);
        // Nothing is requested for the removed output after its removal.
        let report = compositor.finish();
        assert_eq!(report.captured_outputs, vec!["HDMI-A-1".to_string()]);
        assert_eq!((report.copies, report.frames_destroyed), (1, 1));
    }

    #[test]
    fn removing_another_output_leaves_capture_working() {
        let (compositor, connection) =
            FakeCompositor::spawn(padded_scenario(vec![Reply::Ready { y_invert: false }; 2]));
        let mut client = ScreencopyClient::from_connection(connection, "HDMI-A-1").unwrap();
        client.capture().unwrap();

        compositor.remove_output("DP-1");
        settle(&mut client);
        let gray = client.capture().unwrap();
        settle(&mut client);

        assert_eq!(gray_rows(&gray), vec![TOP_ROW.to_vec(), BOTTOM_ROW.to_vec()]);
        let report = compositor.finish();
        assert_eq!(
            report.captured_outputs,
            vec!["HDMI-A-1".to_string(), "HDMI-A-1".to_string()]
        );
        assert_eq!(report.copies, 2);
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

    /// Captures the real output named by `AUTOCLICK_LIVE_OUTPUT` from the active
    /// session and reports timing. It only reads pixels: no pointer is created
    /// and nothing is clicked.
    ///
    /// `AUTOCLICK_LIVE_OUTPUT=HDMI-A-1 cargo test --test unit \
    ///     live_capture_of_configured_output -- --ignored --nocapture`
    #[test]
    #[ignore = "needs a live wlr-screencopy compositor; set AUTOCLICK_LIVE_OUTPUT"]
    fn live_capture_of_configured_output() {
        const FRAMES: u32 = 10;

        let Some(output) = std::env::var_os("AUTOCLICK_LIVE_OUTPUT") else {
            eprintln!("AUTOCLICK_LIVE_OUTPUT is unset; skipping live capture");
            return;
        };
        let output = output.to_string_lossy().into_owned();
        let mut client = ScreencopyClient::connect(&output).unwrap();

        let mut timings = Vec::new();
        let mut last = None;
        for _ in 0..FRAMES {
            let started = std::time::Instant::now();
            let frame = client.capture().unwrap();
            timings.push(started.elapsed());
            last = Some(frame);
        }
        let last = last.unwrap();

        let min = timings.iter().min().unwrap().as_secs_f64() * 1000.0;
        let avg = timings.iter().sum::<std::time::Duration>().as_secs_f64() * 1000.0
            / f64::from(FRAMES);
        println!(
            "{output}: {}x{} grayscale, {FRAMES} captures, min {min:.2} ms, avg {avg:.2} ms",
            last.cols(),
            last.rows()
        );
        assert_eq!(last.typ(), CV_8UC1);
        assert!(last.cols() > 0 && last.rows() > 0);

        if let Some(path) = std::env::var_os("AUTOCLICK_LIVE_PNG") {
            let path = path.to_string_lossy().into_owned();
            let written =
                opencv::imgcodecs::imwrite(&path, &last, &opencv::core::Vector::new()).unwrap();
            assert!(written, "OpenCV refused to write {path}");
            println!("{output}: last frame written to {path}");
        }
    }

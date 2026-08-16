    use std::collections::VecDeque;

    #[derive(Debug, Clone, PartialEq, Eq)]
    enum Event {
        Motion(u32, u32, u32, u32, u32),
        Frame,
        Button(u32, ButtonState),
        Barrier,
        Cleanup(u32),
    }

    #[derive(Default)]
    struct Recorder {
        events: Vec<Event>,
        fail_at: Option<&'static str>,
    }

    impl VirtualPointerPort for Recorder {
        fn motion_absolute(
            &mut self,
            time: u32,
            x: u32,
            y: u32,
            width: u32,
            height: u32,
        ) -> Result<()> {
            self.events.push(Event::Motion(time, x, y, width, height));
            fail_if(self.fail_at, "motion")
        }

        fn frame(&mut self) -> Result<()> {
            self.events.push(Event::Frame);
            fail_if(self.fail_at, "frame")
        }

        fn left_button(&mut self, time: u32, state: ButtonState) -> Result<()> {
            self.events.push(Event::Button(time, state));
            fail_if(
                self.fail_at,
                match state {
                    ButtonState::Pressed => "press",
                    ButtonState::Released => "release",
                },
            )
        }

        fn barrier(&mut self) -> Result<()> {
            self.events.push(Event::Barrier);
            fail_if(self.fail_at, "barrier")
        }

        fn best_effort_release(&mut self, time: u32) -> Result<()> {
            self.events.push(Event::Cleanup(time));
            fail_if(self.fail_at, "cleanup")
        }
    }

    fn fail_if(fail_at: Option<&str>, step: &str) -> Result<()> {
        if fail_at == Some(step) {
            bail!("{step} failpoint")
        }
        Ok(())
    }

    struct TestClock(VecDeque<u32>);

    impl TestClock {
        fn new(timestamps: impl IntoIterator<Item = u32>) -> Self {
            Self(timestamps.into_iter().collect())
        }
    }

    impl Clock for TestClock {
        fn next_ms(&mut self) -> u32 {
            self.0.pop_front().expect("test clock requires a timestamp")
        }
    }

    fn valid_click() -> PlannedClick {
        PlannedClick {
            rule_index: 7,
            target_template: "target.png".into(),
            output_x: 9,
            output_y: 4,
            extent: ImageExtent {
                width: 10,
                height: 5,
            },
        }
    }

    #[test]
    fn rejects_invalid_clicks_without_requests() {
        for click in [
            PlannedClick {
                extent: ImageExtent {
                    width: 0,
                    height: 5,
                },
                ..valid_click()
            },
            PlannedClick {
                extent: ImageExtent {
                    width: -1,
                    height: 5,
                },
                ..valid_click()
            },
            PlannedClick {
                output_x: -1,
                ..valid_click()
            },
            PlannedClick {
                output_y: 5,
                ..valid_click()
            },
        ] {
            let transaction = ClickTransaction::new(Recorder::default(), TestClock::new([1, 2, 3]));
            let mut transaction = transaction;
            assert!(transaction.execute(&click).is_err());
            assert!(transaction.into_port().events.is_empty());
        }
    }

    #[test]
    fn sends_the_exact_framed_transaction_to_one_port() {
        let mut transaction =
            ClickTransaction::new(Recorder::default(), TestClock::new([10, 11, 12]));
        transaction.execute(&valid_click()).unwrap();

        assert_eq!(
            transaction.into_port().events,
            vec![
                Event::Motion(10, 9, 4, 10, 5),
                Event::Frame,
                Event::Button(11, ButtonState::Pressed),
                Event::Frame,
                Event::Button(12, ButtonState::Released),
                Event::Frame,
                Event::Barrier,
            ]
        );
    }

    #[test]
    fn accepts_monotonic_timestamps_across_u32_wraparound() {
        let mut transaction =
            ClickTransaction::new(Recorder::default(), TestClock::new([u32::MAX - 1, 0, 1]));
        transaction.execute(&valid_click()).unwrap();
    }

    #[test]
    fn motion_failure_short_circuits_before_any_button_request() {
        let mut transaction = ClickTransaction::new(
            Recorder {
                fail_at: Some("motion"),
                ..Recorder::default()
            },
            TestClock::new([1, 2, 3]),
        );
        assert!(transaction.execute(&valid_click()).is_err());
        assert_eq!(
            transaction.into_port().events,
            vec![Event::Motion(1, 9, 4, 10, 5)]
        );
    }

    #[test]
    fn discovery_requires_v2_manager_and_exactly_one_live_seat() {
        let mut discovery = DiscoveryState::new("DP-1");
        assert_eq!(
            discovery.validate().unwrap_err(),
            "virtual-pointer manager is absent"
        );
        discovery.reduce(DiscoveryEvent::ManagerAdvertised(1));
        assert_eq!(
            discovery.validate().unwrap_err(),
            "virtual-pointer manager version 1 is below v2"
        );
        discovery.reduce(DiscoveryEvent::ManagerAdvertised(2));
        assert_eq!(
            discovery.validate().unwrap_err(),
            "expected exactly one live seat, found 0"
        );
        discovery.reduce(DiscoveryEvent::SeatAdded(7));
        discovery.reduce(DiscoveryEvent::OutputName(1, "DP-2".into()));
        assert_eq!(
            discovery.validate().unwrap_err(),
            "configured connector was not found"
        );
        discovery.reduce(DiscoveryEvent::SeatRemoved(7));
        discovery.reduce(DiscoveryEvent::SeatAdded(7));
        discovery.reduce(DiscoveryEvent::SeatAdded(8));
        assert_eq!(
            discovery.validate().unwrap_err(),
            "expected exactly one live seat, found 2"
        );
    }

    #[test]
    fn discovery_accepts_only_a_complete_normal_selected_output() {
        let mut discovery = ready_discovery();
        discovery.reduce(DiscoveryEvent::OutputName(2, "HDMI-A-1".into()));
        discovery.reduce(DiscoveryEvent::OutputTransform(2, Transform::Rotated));
        assert_eq!(discovery.validate(), Ok(()));

        let mut incomplete = DiscoveryState::new("DP-1");
        incomplete.reduce(DiscoveryEvent::ManagerAdvertised(2));
        incomplete.reduce(DiscoveryEvent::SeatAdded(7));
        incomplete.reduce(DiscoveryEvent::OutputName(1, "DP-1".into()));
        incomplete.reduce(DiscoveryEvent::OutputMode(1));
        incomplete.reduce(DiscoveryEvent::OutputScale(1));
        incomplete.reduce(DiscoveryEvent::OutputTransform(1, Transform::Normal));
        assert_eq!(
            incomplete.validate().unwrap_err(),
            "selected output is incomplete"
        );
        incomplete.reduce(DiscoveryEvent::OutputDone(1));
        assert_eq!(incomplete.validate(), Ok(()));
    }

    #[test]
    fn discovery_rejects_duplicate_configured_connector_outputs() {
        let mut discovery = ready_discovery();
        discovery.reduce(DiscoveryEvent::OutputName(2, "DP-1".into()));
        discovery.reduce(DiscoveryEvent::OutputMode(2));
        discovery.reduce(DiscoveryEvent::OutputScale(2));
        discovery.reduce(DiscoveryEvent::OutputTransform(2, Transform::Normal));
        discovery.reduce(DiscoveryEvent::OutputDone(2));

        assert_eq!(
            discovery.validate().unwrap_err(),
            "configured connector is ambiguous: 2 live outputs match"
        );
    }

    #[test]
    fn discovery_reports_all_ambiguous_configured_connector_matches() {
        let mut discovery = ready_discovery();
        for id in [2, 3] {
            discovery.reduce(DiscoveryEvent::OutputName(id, "DP-1".into()));
            discovery.reduce(DiscoveryEvent::OutputMode(id));
            discovery.reduce(DiscoveryEvent::OutputScale(id));
            discovery.reduce(DiscoveryEvent::OutputTransform(id, Transform::Normal));
            discovery.reduce(DiscoveryEvent::OutputDone(id));
        }

        assert_eq!(
            discovery.validate().unwrap_err(),
            "configured connector is ambiguous: 3 live outputs match"
        );
    }

    #[test]
    fn discovery_rejects_selected_non_normal_transform() {
        let mut discovery = ready_discovery();
        discovery.reduce(DiscoveryEvent::OutputTransform(1, Transform::Rotated));
        assert_eq!(
            discovery.validate().unwrap_err(),
            "selected output transform is not Normal"
        );
    }

    #[test]
    fn selected_removal_or_metadata_loss_invalidates_without_rebinding() {
        for event in [
            DiscoveryEvent::ManagerRemoved,
            DiscoveryEvent::SeatRemoved(7),
            DiscoveryEvent::OutputRemoved(1),
            DiscoveryEvent::OutputMetadataLost(1),
        ] {
            let mut discovery = ready_discovery();
            assert_eq!(discovery.validate(), Ok(()));
            discovery.reduce(event);
            assert_eq!(
                discovery.validate().unwrap_err(),
                "selected discovery object was invalidated"
            );
        }
    }

    fn ready_discovery() -> DiscoveryState {
        let mut discovery = DiscoveryState::new("DP-1");
        discovery.reduce(DiscoveryEvent::ManagerAdvertised(2));
        discovery.reduce(DiscoveryEvent::SeatAdded(7));
        discovery.reduce(DiscoveryEvent::OutputName(1, "DP-1".into()));
        discovery.reduce(DiscoveryEvent::OutputMode(1));
        discovery.reduce(DiscoveryEvent::OutputScale(1));
        discovery.reduce(DiscoveryEvent::OutputTransform(1, Transform::Normal));
        discovery.reduce(DiscoveryEvent::OutputDone(1));
        discovery
    }

    #[test]
    fn protocol_substrate_confines_generated_types_without_a_socket() {
        use wayland_client::protocol::wl_registry::WlRegistry;
        use wayland_protocols_wlr::virtual_pointer::v1::client::zwlr_virtual_pointer_manager_v1::ZwlrVirtualPointerManagerV1;

        assert_eq!(super::REQUIRED_MANAGER_VERSION, 2);
        let _: Option<WlRegistry> = None;
        let _: Option<ZwlrVirtualPointerManagerV1> = None;
    }

    #[test]
    fn adapter_state_implements_generated_registry_and_output_dispatch() {
        use wayland_client::{
            protocol::{wl_output::WlOutput, wl_registry::WlRegistry},
            Dispatch,
        };

        fn assert_dispatch<D>()
        where
            D: Dispatch<WlRegistry, ()> + Dispatch<WlOutput, u32>,
        {
        }

        assert_dispatch::<protocol_substrate::AdapterState>();
    }

    #[test]
    fn adapter_reducers_preserve_selected_object_lifecycle() {
        let mut adapter = protocol_substrate::AdapterState::new("DP-1");
        adapter.manager_advertised(2);
        adapter.seat_added(7);
        adapter.output_name(1, "DP-1");
        adapter.output_current_mode(1);
        adapter.output_scale(1);
        adapter.output_normal_transform(1);
        adapter.output_done(1);
        assert_eq!(adapter.validate(), Ok(()));

        adapter.output_metadata_lost(1);
        assert_eq!(
            adapter.validate().unwrap_err(),
            "selected discovery object was invalidated"
        );
    }

    #[test]
    fn adapter_reducers_invalidate_only_selected_manager_or_seat() {
        let mut manager = ready_adapter();
        manager.manager_removed();
        assert_eq!(
            manager.validate().unwrap_err(),
            "selected discovery object was invalidated"
        );

        let mut seat = ready_adapter();
        seat.seat_removed(7);
        assert_eq!(
            seat.validate().unwrap_err(),
            "selected discovery object was invalidated"
        );
    }

    fn ready_adapter() -> protocol_substrate::AdapterState {
        let mut adapter = protocol_substrate::AdapterState::new("DP-1");
        adapter.manager_advertised(2);
        adapter.seat_added(7);
        adapter.output_name(1, "DP-1");
        adapter.output_current_mode(1);
        adapter.output_scale(1);
        adapter.output_normal_transform(1);
        adapter.output_done(1);
        assert_eq!(adapter.validate(), Ok(()));
        adapter
    }

    #[test]
    fn selected_pointer_lifecycle_rejects_requests_after_invalidation() {
        let mut lifecycle = protocol_substrate::SelectedPointerLifecycle::active();
        assert_eq!(lifecycle.ensure_active(), Ok(()));

        lifecycle.invalidate("selected output 1 was removed");
        assert_eq!(
            lifecycle.ensure_active().unwrap_err(),
            "selected virtual pointer is invalidated: selected output 1 was removed"
        );
    }

    #[test]
    fn semantic_adapter_stops_a_request_when_already_invalidated() {
        let mut adapter = ready_adapter();
        adapter.output_metadata_lost(1);
        let mut requests = 0;

        let error = protocol_substrate::semantic_request(
            &mut adapter,
            |_| Ok(()),
            || {
                requests += 1;
                Ok(())
            },
        )
        .unwrap_err();

        assert_eq!(
            error.to_string(),
            "selected discovery object was invalidated"
        );
        assert_eq!(requests, 0);
    }

    #[test]
    fn semantic_adapter_stops_a_request_invalidated_during_dispatch() {
        let mut adapter = ready_adapter();
        let mut requests = 0;

        let error = protocol_substrate::semantic_request(
            &mut adapter,
            |state| {
                state.output_metadata_lost(1);
                Ok(())
            },
            || {
                requests += 1;
                Ok(())
            },
        )
        .unwrap_err();

        assert_eq!(
            error.to_string(),
            "selected discovery object was invalidated"
        );
        assert_eq!(requests, 0);
    }

    #[test]
    fn semantic_adapter_stops_a_barrier_after_dispatch_invalidation() {
        let mut adapter = ready_adapter();
        let mut barriers = 0;

        let error = protocol_substrate::semantic_barrier(
            &mut adapter,
            |state| {
                state.output_metadata_lost(1);
                Ok(())
            },
            || {
                barriers += 1;
                Ok(())
            },
        )
        .unwrap_err();

        assert_eq!(
            error.to_string(),
            "selected discovery object was invalidated"
        );
        assert_eq!(barriers, 0);
    }

    #[test]
    fn generated_pointer_adapter_uses_left_button_and_exposes_barrier_and_close_signatures() {
        assert_eq!(protocol_substrate::ProtocolSubstrate::LEFT_BUTTON, 0x110);
    }

    #[test]
    fn generated_client_fails_before_requests_and_closes_idempotently_after_manager_removal() {
        use std::{
            os::unix::net::UnixStream,
            sync::{mpsc, Arc},
            thread,
        };
        use wayland_client::Connection;
        use wayland_protocols_wlr::virtual_pointer::v1::server::{
            zwlr_virtual_pointer_manager_v1::{self, ZwlrVirtualPointerManagerV1},
            zwlr_virtual_pointer_v1::{self, ZwlrVirtualPointerV1},
        };
        use wayland_server::{
            protocol::{
                wl_output::{self, WlOutput},
                wl_seat::WlSeat,
            },
            Client, DataInit, Dispatch, Display, DisplayHandle, GlobalDispatch, New,
        };

        #[derive(Default)]
        struct ServerState(Vec<String>);
        impl GlobalDispatch<WlSeat, ()> for ServerState {
            fn bind(
                _: &mut Self,
                _: &DisplayHandle,
                _: &Client,
                resource: New<WlSeat>,
                _: &(),
                data_init: &mut DataInit<'_, Self>,
            ) {
                data_init.init(resource, ());
            }
        }
        #[rustfmt::skip]
        impl Dispatch<WlSeat, ()> for ServerState {
            fn request(_: &mut Self, _: &Client, _: &WlSeat, _: wayland_server::protocol::wl_seat::Request, _: &(), _: &DisplayHandle, _: &mut DataInit<'_, Self>) {}
        }
        impl GlobalDispatch<WlOutput, ()> for ServerState {
            fn bind(
                _: &mut Self,
                _: &DisplayHandle,
                _: &Client,
                resource: New<WlOutput>,
                _: &(),
                data_init: &mut DataInit<'_, Self>,
            ) {
                let output = data_init.init(resource, ());
                output.geometry(
                    0,
                    0,
                    800,
                    600,
                    wl_output::Subpixel::Unknown,
                    "test".into(),
                    "Normal".into(),
                    wl_output::Transform::Normal,
                );
                output.mode(wl_output::Mode::Current, 800, 600, 60_000);
                output.scale(1);
                output.name("DP-1".into());
                output.done();
            }
        }
        #[rustfmt::skip]
        impl Dispatch<WlOutput, ()> for ServerState {
            fn request(_: &mut Self, _: &Client, _: &WlOutput, _: wl_output::Request, _: &(), _: &DisplayHandle, _: &mut DataInit<'_, Self>) {}
        }
        impl GlobalDispatch<ZwlrVirtualPointerManagerV1, ()> for ServerState {
            fn bind(
                _: &mut Self,
                _: &DisplayHandle,
                _: &Client,
                resource: New<ZwlrVirtualPointerManagerV1>,
                _: &(),
                data_init: &mut DataInit<'_, Self>,
            ) {
                data_init.init(resource, ());
            }
        }
        impl Dispatch<ZwlrVirtualPointerManagerV1, ()> for ServerState {
            fn request(
                _: &mut Self,
                _: &Client,
                _: &ZwlrVirtualPointerManagerV1,
                request: zwlr_virtual_pointer_manager_v1::Request,
                _: &(),
                _: &DisplayHandle,
                data_init: &mut DataInit<'_, Self>,
            ) {
                if let zwlr_virtual_pointer_manager_v1::Request::CreateVirtualPointerWithOutput {
                    id,
                    ..
                } = request
                {
                    data_init.init(id, ());
                }
            }
        }
        impl Dispatch<ZwlrVirtualPointerV1, ()> for ServerState {
            fn request(
                state: &mut Self,
                _: &Client,
                _: &ZwlrVirtualPointerV1,
                request: zwlr_virtual_pointer_v1::Request,
                _: &(),
                _: &DisplayHandle,
                _: &mut DataInit<'_, Self>,
            ) {
                use zwlr_virtual_pointer_v1::Request::*;
                state.0.push(
                    match request {
                        MotionAbsolute { .. } => "motion",
                        Frame => "frame",
                        Button {
                            state:
                                wayland_server::WEnum::Value(
                                    wayland_server::protocol::wl_pointer::ButtonState::Pressed,
                                ),
                            ..
                        } => "press",
                        Button { .. } => "release",
                        Destroy => "destroy",
                        _ => "unexpected",
                    }
                    .into(),
                );
            }
        }

        enum Command {
            RemoveManager,
            Stop,
        }

        let (client, server) = UnixStream::pair().unwrap();
        let (done, received) = mpsc::channel();
        let (command, commands) = mpsc::channel();
        let (removal_sent, removed) = mpsc::channel();
        let server = thread::spawn(move || {
            let mut display = Display::<ServerState>::new().unwrap();
            let mut handle = display.handle();
            handle.create_global::<ServerState, WlSeat, _>(1, ());
            handle.create_global::<ServerState, WlOutput, _>(4, ());
            let mut manager =
                Some(handle.create_global::<ServerState, ZwlrVirtualPointerManagerV1, _>(2, ()));
            server.set_nonblocking(true).unwrap();
            handle.insert_client(server, Arc::new(())).unwrap();
            let mut state = ServerState::default();
            loop {
                match commands.try_recv() {
                    Ok(Command::RemoveManager) => {
                        handle.remove_global::<ServerState>(manager.take().unwrap());
                        display.flush_clients().unwrap();
                        removal_sent.send(()).unwrap();
                    }
                    Ok(Command::Stop) | Err(mpsc::TryRecvError::Disconnected) => break,
                    Err(mpsc::TryRecvError::Empty) => {}
                }
                if display.dispatch_clients(&mut state).is_ok() {
                    display.flush_clients().unwrap();
                } else {
                    thread::yield_now();
                }
            }
            done.send(state.0).unwrap();
        });

        let connection = Connection::from_socket(client).unwrap();
        let mut substrate =
            protocol_substrate::ProtocolSubstrate::from_connection(connection, "DP-1").unwrap();
        ClickTransaction::new(&mut substrate, TestClock::new([10, 11, 12]))
            .execute(&valid_click())
            .unwrap();
        command.send(Command::RemoveManager).unwrap();
        removed.recv().unwrap();
        let error = ClickTransaction::new(&mut substrate, TestClock::new([13, 14, 15]))
            .execute(&valid_click())
            .unwrap_err();
        assert!(error
            .to_string()
            .contains("selected discovery object was invalidated"));

        // `Drop for WaylandPointerBackend` calls `close` unconditionally, so closing
        // must stay safe on this already-degraded connection and after an explicit
        // close. Exactly one `destroy` reaches the server across both calls: the
        // second finds the pointer already taken and emits nothing.
        substrate.close().unwrap();
        substrate.close().unwrap();

        command.send(Command::Stop).unwrap();
        assert_eq!(
            received.recv().unwrap(),
            ["motion", "frame", "press", "frame", "release", "frame", "destroy"]
        );
        server.join().unwrap();
    }

    #[test]
    fn post_press_failure_attempts_one_cleanup_without_retrying_the_click() {
        let mut transaction = ClickTransaction::new(
            Recorder {
                fail_at: Some("barrier"),
                ..Recorder::default()
            },
            TestClock::new([1, 2, 3, 4]),
        );
        let error = transaction.execute(&valid_click()).unwrap_err();
        let port = transaction.into_port();

        assert!(error.to_string().contains("button delivery is unknown"));
        assert_eq!(
            port.events,
            vec![
                Event::Motion(1, 9, 4, 10, 5),
                Event::Frame,
                Event::Button(2, ButtonState::Pressed),
                Event::Frame,
                Event::Button(3, ButtonState::Released),
                Event::Frame,
                Event::Barrier,
                Event::Cleanup(4),
            ]
        );
    }

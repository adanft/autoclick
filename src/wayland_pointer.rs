use anyhow::{anyhow, bail, Result};

/// Private generated-protocol boundary. It is deliberately not constructed by the runtime yet.
mod protocol_substrate {
    use anyhow::Context;
    use wayland_client::{
        protocol::{wl_output::WlOutput, wl_registry, wl_registry::WlRegistry, wl_seat::WlSeat},
        Connection, Dispatch, EventQueue, Proxy, QueueHandle,
    };
    use wayland_protocols_wlr::virtual_pointer::v1::client::{
        zwlr_virtual_pointer_manager_v1::ZwlrVirtualPointerManagerV1,
        zwlr_virtual_pointer_v1::ZwlrVirtualPointerV1,
    };

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub(super) struct ManagerGlobal {
        pub(super) name: u32,
        pub(super) advertised_version: u32,
    }

    #[derive(Default)]
    pub(super) struct SelectedPointerLifecycle {
        invalidation: Option<String>,
    }

    impl SelectedPointerLifecycle {
        pub(super) fn active() -> Self {
            Self::default()
        }

        pub(super) fn invalidate(&mut self, reason: impl Into<String>) {
            self.invalidation = Some(reason.into());
        }

        pub(super) fn ensure_active(&self) -> std::result::Result<(), String> {
            self.invalidation.as_ref().map_or(Ok(()), |reason| {
                Err(format!("selected virtual pointer is invalidated: {reason}"))
            })
        }
    }

    #[derive(Default)]
    pub(super) struct RegistryState {
        pub(super) manager: Option<ManagerGlobal>,
    }

    impl RegistryState {
        pub(super) fn record_global(&mut self, name: u32, interface: &str, version: u32) {
            if interface == ZwlrVirtualPointerManagerV1::interface().name {
                self.manager = Some(ManagerGlobal {
                    name,
                    advertised_version: version,
                });
            }
        }
    }

    impl Dispatch<WlRegistry, ()> for RegistryState {
        fn event(
            state: &mut Self,
            _: &WlRegistry,
            event: wl_registry::Event,
            _: &(),
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
            match event {
                wl_registry::Event::Global {
                    name,
                    interface,
                    version,
                } => state.record_global(name, &interface, version),
                wl_registry::Event::GlobalRemove { name }
                    if state.manager.is_some_and(|manager| manager.name == name) =>
                {
                    state.manager = None;
                }
                _ => {}
            }
        }
    }

    wayland_client::delegate_noop!(RegistryState: ignore WlSeat);
    wayland_client::delegate_noop!(RegistryState: ignore WlOutput);
    wayland_client::delegate_noop!(RegistryState: ignore ZwlrVirtualPointerManagerV1);
    wayland_client::delegate_noop!(RegistryState: ignore ZwlrVirtualPointerV1);

    /// Callback-owned selected-object state. Proxy user data is a stable registry ID.
    pub(super) struct AdapterState {
        discovery: super::DiscoveryState,
        manager_global: Option<u32>,
        manager: Option<ZwlrVirtualPointerManagerV1>,
        seats: std::collections::BTreeMap<u32, WlSeat>,
        outputs: std::collections::BTreeMap<u32, WlOutput>,
        lifecycle: SelectedPointerLifecycle,
    }

    #[allow(dead_code)]
    impl AdapterState {
        pub(super) fn new(connector: &str) -> Self {
            Self {
                discovery: super::DiscoveryState::new(connector),
                manager_global: None,
                manager: None,
                seats: Default::default(),
                outputs: Default::default(),
                lifecycle: SelectedPointerLifecycle::active(),
            }
        }

        pub(super) fn manager_advertised(&mut self, version: u32) {
            self.discovery
                .reduce(super::DiscoveryEvent::ManagerAdvertised(version));
        }

        fn manager_global(&mut self, id: u32, version: u32) {
            self.manager_global = Some(id);
            self.manager_advertised(version);
        }

        pub(super) fn manager_removed(&mut self) {
            self.discovery.reduce(super::DiscoveryEvent::ManagerRemoved);
        }

        pub(super) fn seat_added(&mut self, id: u32) {
            self.discovery.reduce(super::DiscoveryEvent::SeatAdded(id));
        }

        pub(super) fn seat_removed(&mut self, id: u32) {
            self.discovery
                .reduce(super::DiscoveryEvent::SeatRemoved(id));
        }

        pub(super) fn output_name(&mut self, id: u32, name: impl Into<String>) {
            self.discovery
                .reduce(super::DiscoveryEvent::OutputName(id, name.into()));
        }

        pub(super) fn output_current_mode(&mut self, id: u32) {
            self.discovery.reduce(super::DiscoveryEvent::OutputMode(id));
        }

        pub(super) fn output_scale(&mut self, id: u32) {
            self.discovery
                .reduce(super::DiscoveryEvent::OutputScale(id));
        }

        pub(super) fn output_normal_transform(&mut self, id: u32) {
            self.discovery
                .reduce(super::DiscoveryEvent::OutputTransform(
                    id,
                    super::Transform::Normal,
                ));
        }

        pub(super) fn output_done(&mut self, id: u32) {
            self.discovery.reduce(super::DiscoveryEvent::OutputDone(id));
        }

        pub(super) fn output_metadata_lost(&mut self, id: u32) {
            self.discovery
                .reduce(super::DiscoveryEvent::OutputMetadataLost(id));
        }

        pub(super) fn validate(&mut self) -> std::result::Result<(), String> {
            self.lifecycle.ensure_active()?;
            self.discovery.validate()
        }
    }

    /// Runs queued adapter dispatch before a semantic request and denies stale pointers.
    /// The closure seam permits hardware-independent lifecycle tests.
    pub(super) fn semantic_request<D, R>(
        state: &mut AdapterState,
        dispatch: D,
        request: R,
    ) -> anyhow::Result<()>
    where
        D: FnOnce(&mut AdapterState) -> anyhow::Result<()>,
        R: FnOnce() -> anyhow::Result<()>,
    {
        dispatch(state)?;
        state.validate().map_err(anyhow::Error::msg)?;
        request()
    }

    /// Checks lifecycle again after a synchronous barrier dispatch.
    pub(super) fn semantic_barrier<D, R>(
        state: &mut AdapterState,
        dispatch: D,
        complete: R,
    ) -> anyhow::Result<()>
    where
        D: FnOnce(&mut AdapterState) -> anyhow::Result<()>,
        R: FnOnce() -> anyhow::Result<()>,
    {
        semantic_request(state, dispatch, complete)
    }

    impl AdapterState {
        fn selected_proxies(
            &self,
        ) -> anyhow::Result<(ZwlrVirtualPointerManagerV1, WlSeat, WlOutput)> {
            let seat = self
                .discovery
                .selected_seat
                .ok_or_else(|| anyhow::anyhow!("selected seat is unavailable"))?;
            let output = self
                .discovery
                .selected_output
                .ok_or_else(|| anyhow::anyhow!("selected output is unavailable"))?;
            Ok((
                self.manager
                    .clone()
                    .ok_or_else(|| anyhow::anyhow!("selected manager is unavailable"))?,
                self.seats
                    .get(&seat)
                    .cloned()
                    .ok_or_else(|| anyhow::anyhow!("selected seat is unavailable"))?,
                self.outputs
                    .get(&output)
                    .cloned()
                    .ok_or_else(|| anyhow::anyhow!("selected output is unavailable"))?,
            ))
        }
    }

    impl Dispatch<WlRegistry, ()> for AdapterState {
        fn event(
            state: &mut Self,
            registry: &WlRegistry,
            event: wl_registry::Event,
            _: &(),
            _: &Connection,
            qh: &QueueHandle<Self>,
        ) {
            match event {
                wl_registry::Event::Global {
                    name,
                    interface,
                    version,
                } if interface == ZwlrVirtualPointerManagerV1::interface().name => {
                    state.manager_global(name, version);
                    state.manager = Some(registry.bind(name, version.min(2), qh, ()));
                }
                wl_registry::Event::Global {
                    name,
                    interface,
                    version,
                } if interface == WlSeat::interface().name => {
                    state.seat_added(name);
                    state
                        .seats
                        .insert(name, registry.bind(name, version.min(1), qh, ()));
                }
                wl_registry::Event::Global {
                    name,
                    interface,
                    version,
                } if interface == WlOutput::interface().name => {
                    state
                        .outputs
                        .insert(name, registry.bind(name, version.min(4), qh, name));
                }
                wl_registry::Event::GlobalRemove { name } if state.manager_global == Some(name) => {
                    state.manager_removed();
                    state.manager = None;
                }
                wl_registry::Event::GlobalRemove { name } => {
                    state.seats.remove(&name);
                    state.outputs.remove(&name);
                    state.seat_removed(name);
                    state
                        .discovery
                        .reduce(super::DiscoveryEvent::OutputRemoved(name));
                }
                _ => {}
            }
        }
    }

    impl Dispatch<WlOutput, u32> for AdapterState {
        fn event(
            state: &mut Self,
            _: &WlOutput,
            event: wayland_client::protocol::wl_output::Event,
            id: &u32,
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
            use wayland_client::protocol::wl_output::{Event, Transform};
            use wayland_client::WEnum;

            match event {
                Event::Name { name } => state.output_name(*id, name),
                Event::Mode {
                    flags: WEnum::Value(flags),
                    ..
                } if flags.contains(wayland_client::protocol::wl_output::Mode::Current) => {
                    state.output_current_mode(*id);
                }
                Event::Scale { .. } => state.output_scale(*id),
                Event::Geometry { transform, .. } => match transform {
                    WEnum::Value(Transform::Normal) => state.output_normal_transform(*id),
                    _ => state
                        .discovery
                        .reduce(super::DiscoveryEvent::OutputTransform(
                            *id,
                            super::Transform::Rotated,
                        )),
                },
                Event::Done => state.output_done(*id),
                _ => {}
            }
        }
    }

    wayland_client::delegate_noop!(AdapterState: ignore WlSeat);
    wayland_client::delegate_noop!(AdapterState: ignore ZwlrVirtualPointerManagerV1);
    wayland_client::delegate_noop!(AdapterState: ignore ZwlrVirtualPointerV1);

    /// Owns the synchronous Wayland primitives without borrowing from itself.
    #[allow(dead_code)]
    pub(super) struct ProtocolSubstrate {
        connection: Connection,
        event_queue: EventQueue<AdapterState>,
        registry: WlRegistry,
        state: AdapterState,
        pointer: Option<ZwlrVirtualPointerV1>,
    }

    #[allow(dead_code)]
    impl ProtocolSubstrate {
        pub(super) const MANAGER_VERSION: u32 = 2;
        pub(super) const LEFT_BUTTON: u32 = 0x110;

        pub(super) fn connect(connector: &str) -> anyhow::Result<Self> {
            Self::from_connection(Connection::connect_to_env()?, connector)
        }

        pub(super) fn from_connection(
            connection: Connection,
            connector: &str,
        ) -> anyhow::Result<Self> {
            let event_queue = connection.new_event_queue();
            let registry = connection.display().get_registry(&event_queue.handle(), ());
            let mut substrate = Self {
                connection,
                event_queue,
                registry,
                state: AdapterState::new(connector),
                pointer: None,
            };
            substrate
                .event_queue
                .roundtrip(&mut substrate.state)
                .context("Wayland registry discovery roundtrip failed")?;
            // Registry globals bind proxies during the first roundtrip; a second boundary
            // receives their wl_output metadata before selected-output validation.
            substrate
                .event_queue
                .roundtrip(&mut substrate.state)
                .context("Wayland output metadata discovery roundtrip failed")?;
            substrate
                .state
                .validate()
                .map_err(|error| anyhow::anyhow!("Wayland discovery validation failed: {error}"))?;
            let (manager, seat, output) = substrate.state.selected_proxies()?;
            substrate.pointer = Some(manager.create_virtual_pointer_with_output(
                Some(&seat),
                Some(&output),
                &substrate.event_queue.handle(),
                (),
            ));
            Ok(substrate)
        }

        fn before_request(&mut self) -> anyhow::Result<&ZwlrVirtualPointerV1> {
            let (event_queue, state) = (&mut self.event_queue, &mut self.state);
            semantic_request(
                state,
                |state| event_queue.roundtrip(state).map(|_| ()).map_err(Into::into),
                || Ok(()),
            )?;
            self.pointer
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("selected virtual pointer is closed"))
        }

        pub(super) fn motion_absolute(
            &mut self,
            time: u32,
            x: u32,
            y: u32,
            width: u32,
            height: u32,
        ) -> anyhow::Result<()> {
            self.before_request()?
                .motion_absolute(time, x, y, width, height);
            Ok(())
        }

        pub(super) fn frame(&mut self) -> anyhow::Result<()> {
            self.before_request()?.frame();
            Ok(())
        }

        pub(super) fn left_button(
            &mut self,
            time: u32,
            state: super::ButtonState,
        ) -> anyhow::Result<()> {
            use wayland_client::protocol::wl_pointer::ButtonState;
            let state = match state {
                super::ButtonState::Pressed => ButtonState::Pressed,
                super::ButtonState::Released => ButtonState::Released,
            };
            self.before_request()?
                .button(time, Self::LEFT_BUTTON, state);
            Ok(())
        }

        pub(super) fn barrier(&mut self) -> anyhow::Result<()> {
            self.before_request()?;
            self.connection.flush()?;
            let (event_queue, state) = (&mut self.event_queue, &mut self.state);
            semantic_barrier(
                state,
                |state| event_queue.roundtrip(state).map(|_| ()).map_err(Into::into),
                || Ok(()),
            )
        }

        pub(super) fn best_effort_release(&mut self, time: u32) -> anyhow::Result<()> {
            self.left_button(time, super::ButtonState::Released)?;
            self.frame()?;
            self.connection.flush()?;
            Ok(())
        }

        pub(super) fn close(&mut self) -> anyhow::Result<()> {
            self.state.lifecycle.invalidate("backend was closed");
            if let Some(pointer) = self.pointer.take() {
                pointer.destroy();
            }
            if let Some(manager) = self.state.manager.take() {
                manager.destroy();
            }
            self.connection.flush()?;
            Ok(())
        }

        pub(super) fn assert_generated_signatures() {
            fn assert_proxy<P: Proxy>() {}
            let _: fn(&Connection) -> EventQueue<AdapterState> = Connection::new_event_queue;
            let _: fn(
                &WlRegistry,
                u32,
                u32,
                &QueueHandle<RegistryState>,
                (),
            ) -> ZwlrVirtualPointerManagerV1 = WlRegistry::bind;
            let _: fn(&WlRegistry, u32, u32, &QueueHandle<RegistryState>, ()) -> WlSeat =
                WlRegistry::bind;
            let _: fn(&WlRegistry, u32, u32, &QueueHandle<RegistryState>, ()) -> WlOutput =
                WlRegistry::bind;
            assert_proxy::<WlRegistry>();
            assert_proxy::<WlSeat>();
            assert_proxy::<WlOutput>();
            assert_proxy::<ZwlrVirtualPointerManagerV1>();
            assert_proxy::<ZwlrVirtualPointerV1>();
        }

        pub(super) fn assert_pointer_request_signatures() {
            use wayland_client::protocol::wl_pointer::ButtonState;
            let _: fn(&ZwlrVirtualPointerV1, u32, u32, u32, u32, u32) =
                ZwlrVirtualPointerV1::motion_absolute;
            let _: fn(&ZwlrVirtualPointerV1) = ZwlrVirtualPointerV1::frame;
            let _: fn(&ZwlrVirtualPointerV1, u32, u32, ButtonState) = ZwlrVirtualPointerV1::button;
            let _: fn(&ZwlrVirtualPointerV1) = ZwlrVirtualPointerV1::destroy;
            let _: fn(&ZwlrVirtualPointerManagerV1) = ZwlrVirtualPointerManagerV1::destroy;
        }

        pub(super) fn assert_owns_adapter_event_queue() {
            fn assert_queue(_: &EventQueue<AdapterState>) {}
            let _: fn(&ProtocolSubstrate) = |substrate| assert_queue(&substrate.event_queue);
        }
    }

    impl super::VirtualPointerPort for &mut ProtocolSubstrate {
        fn motion_absolute(
            &mut self,
            time: u32,
            x: u32,
            y: u32,
            width: u32,
            height: u32,
        ) -> anyhow::Result<()> {
            ProtocolSubstrate::motion_absolute(self, time, x, y, width, height)
        }

        fn frame(&mut self) -> anyhow::Result<()> {
            ProtocolSubstrate::frame(self)
        }

        fn left_button(&mut self, time: u32, state: super::ButtonState) -> anyhow::Result<()> {
            ProtocolSubstrate::left_button(self, time, state)
        }

        fn barrier(&mut self) -> anyhow::Result<()> {
            ProtocolSubstrate::barrier(self)
        }

        fn best_effort_release(&mut self, time: u32) -> anyhow::Result<()> {
            ProtocolSubstrate::best_effort_release(self, time)
        }
    }
}

#[cfg(test)]
use protocol_substrate::ProtocolSubstrate;

/// Dimensions of the image from which an output-local click was planned.
#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct ImageExtent {
    pub width: i32,
    pub height: i32,
}

/// A click target expressed in coordinates local to the captured output image.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedClick {
    pub rule_index: usize,
    pub target_template: String,
    pub output_x: i32,
    pub output_y: i32,
    pub extent: ImageExtent,
}

/// Executes a validated click plan.
pub trait ClickExecutor {
    fn click(&mut self, click: &PlannedClick) -> Result<()>;
}

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ButtonState {
    Pressed,
    Released,
}

/// Semantic operations exposed by the future generated Wayland adapter.
#[allow(dead_code)]
trait VirtualPointerPort {
    fn motion_absolute(
        &mut self,
        time_ms: u32,
        x: u32,
        y: u32,
        x_extent: u32,
        y_extent: u32,
    ) -> Result<()>;
    fn frame(&mut self) -> Result<()>;
    fn left_button(&mut self, time_ms: u32, state: ButtonState) -> Result<()>;
    fn barrier(&mut self) -> Result<()>;
    fn best_effort_release(&mut self, time_ms: u32) -> Result<()>;
}

#[allow(dead_code)]
trait Clock {
    fn next_ms(&mut self) -> u32;
}

/// Isolated, inactive transaction core for a persistent virtual pointer.
#[allow(dead_code)]
struct ClickTransaction<P, C> {
    port: P,
    clock: C,
    previous_timestamp: Option<u32>,
}

#[allow(dead_code)]
impl<P, C> ClickTransaction<P, C>
where
    P: VirtualPointerPort,
    C: Clock,
{
    fn new(port: P, clock: C) -> Self {
        Self {
            port,
            clock,
            previous_timestamp: None,
        }
    }

    fn execute(&mut self, click: &PlannedClick) -> Result<()> {
        let (x, y, width, height) = validate_click(click)?;
        let motion_time = self.next_timestamp()?;
        self.port
            .motion_absolute(motion_time, x, y, width, height)
            .map_err(|error| anyhow!("virtual-pointer motion failed: {error}"))?;
        self.port
            .frame()
            .map_err(|error| anyhow!("virtual-pointer motion frame failed: {error}"))?;

        let press_time = self.next_timestamp()?;
        if let Err(error) = self.port.left_button(press_time, ButtonState::Pressed) {
            return Err(self.fail_after_press(error));
        }
        if let Err(error) = self.port.frame() {
            return Err(self.fail_after_press(error));
        }

        let release_time = self.next_timestamp()?;
        if let Err(error) = self.port.left_button(release_time, ButtonState::Released) {
            return Err(self.fail_after_press(error));
        }
        if let Err(error) = self.port.frame() {
            return Err(self.fail_after_press(error));
        }
        if let Err(error) = self.port.barrier() {
            return Err(self.fail_after_press(error));
        }

        Ok(())
    }

    fn into_port(self) -> P {
        self.port
    }

    fn next_timestamp(&mut self) -> Result<u32> {
        let timestamp = self.clock.next_ms();
        if let Some(previous) = self.previous_timestamp {
            if timestamp.wrapping_sub(previous) > i32::MAX as u32 {
                bail!("virtual-pointer clock moved backward outside 32-bit wraparound semantics");
            }
        }
        self.previous_timestamp = Some(timestamp);
        Ok(timestamp)
    }

    fn fail_after_press(&mut self, primary: anyhow::Error) -> anyhow::Error {
        let cleanup_time = self.clock.next_ms();
        match self.port.best_effort_release(cleanup_time) {
            Ok(()) => anyhow!(
                "virtual-pointer transaction failed after press; button delivery is unknown: {primary}"
            ),
            Err(cleanup) => anyhow!(
                "virtual-pointer transaction failed after press; button delivery is unknown: {primary}; best-effort release failed: {cleanup}"
            ),
        }
    }
}

impl<P, C> ClickExecutor for ClickTransaction<P, C>
where
    P: VirtualPointerPort,
    C: Clock,
{
    fn click(&mut self, click: &PlannedClick) -> Result<()> {
        self.execute(click)
    }
}

#[allow(dead_code)]
fn validate_click(click: &PlannedClick) -> Result<(u32, u32, u32, u32)> {
    if click.extent.width <= 0 || click.extent.height <= 0 {
        bail!(
            "invalid captured image extent {}x{}",
            click.extent.width,
            click.extent.height
        );
    }
    if click.output_x < 0 || click.output_y < 0 {
        bail!(
            "invalid output-local coordinate ({}, {})",
            click.output_x,
            click.output_y
        );
    }
    if click.output_x >= click.extent.width || click.output_y >= click.extent.height {
        bail!(
            "output-local coordinate ({}, {}) is outside {}x{}",
            click.output_x,
            click.output_y,
            click.extent.width,
            click.extent.height
        );
    }

    Ok((
        click.output_x as u32,
        click.output_y as u32,
        click.extent.width as u32,
        click.extent.height as u32,
    ))
}

#[allow(dead_code)]
#[derive(Clone, Copy, PartialEq, Eq)]
enum Transform {
    Normal,
    Rotated,
}

#[allow(dead_code)]
enum DiscoveryEvent {
    ManagerAdvertised(u32),
    ManagerRemoved,
    SeatAdded(u32),
    SeatRemoved(u32),
    OutputName(u32, String),
    OutputMode(u32),
    OutputScale(u32),
    OutputTransform(u32, Transform),
    OutputDone(u32),
    OutputRemoved(u32),
    OutputMetadataLost(u32),
}

#[allow(dead_code)]
#[derive(Default)]
struct OutputState {
    name: Option<String>,
    mode: bool,
    scale: bool,
    transform: Option<Transform>,
    done: bool,
}

#[allow(dead_code)]
struct DiscoveryState {
    connector: String,
    manager: Option<u32>,
    seats: std::collections::BTreeSet<u32>,
    outputs: std::collections::BTreeMap<u32, OutputState>,
    selected_seat: Option<u32>,
    selected_output: Option<u32>,
    manager_selected: bool,
    invalidated: bool,
}

#[allow(dead_code)]
impl DiscoveryState {
    fn new(connector: &str) -> Self {
        Self {
            connector: connector.into(),
            manager: None,
            seats: Default::default(),
            outputs: Default::default(),
            selected_seat: None,
            selected_output: None,
            manager_selected: false,
            invalidated: false,
        }
    }

    fn reduce(&mut self, event: DiscoveryEvent) {
        match event {
            DiscoveryEvent::ManagerAdvertised(version) => self.manager = Some(version),
            DiscoveryEvent::ManagerRemoved => {
                if self.manager_selected {
                    self.invalidated = true;
                }
                self.manager = None;
            }
            DiscoveryEvent::SeatAdded(id) => {
                self.seats.insert(id);
            }
            DiscoveryEvent::SeatRemoved(id) => {
                self.seats.remove(&id);
                if self.selected_seat == Some(id) {
                    self.invalidated = true;
                }
            }
            DiscoveryEvent::OutputRemoved(id) => {
                self.outputs.remove(&id);
                if self.selected_output == Some(id) {
                    self.invalidated = true;
                }
            }
            DiscoveryEvent::OutputMetadataLost(id) => {
                self.outputs.entry(id).or_default().done = false;
                if self.selected_output == Some(id) {
                    self.invalidated = true;
                }
            }
            DiscoveryEvent::OutputName(id, name) => {
                self.outputs.entry(id).or_default().name = Some(name)
            }
            DiscoveryEvent::OutputMode(id) => self.outputs.entry(id).or_default().mode = true,
            DiscoveryEvent::OutputScale(id) => self.outputs.entry(id).or_default().scale = true,
            DiscoveryEvent::OutputTransform(id, transform) => {
                self.outputs.entry(id).or_default().transform = Some(transform)
            }
            DiscoveryEvent::OutputDone(id) => self.outputs.entry(id).or_default().done = true,
        }
    }

    fn validate(&mut self) -> std::result::Result<(), String> {
        if self.invalidated {
            return Err("selected discovery object was invalidated".into());
        }
        let version = self.manager.ok_or("virtual-pointer manager is absent")?;
        if version < 2 {
            return Err(format!(
                "virtual-pointer manager version {version} is below v2"
            ));
        }
        if self.seats.len() != 1 {
            return Err(format!(
                "expected exactly one live seat, found {}",
                self.seats.len()
            ));
        }
        let selected = match self.selected_output {
            Some(id) => id,
            None => {
                let matching_outputs: Vec<_> = self
                    .outputs
                    .iter()
                    .filter_map(|(&id, output)| {
                        (output.name.as_deref() == Some(&self.connector)).then_some(id)
                    })
                    .collect();
                match matching_outputs.as_slice() {
                    [] => return Err("configured connector was not found".into()),
                    [id] => *id,
                    matches => {
                        return Err(format!(
                            "configured connector is ambiguous: {} live outputs match",
                            matches.len()
                        ));
                    }
                }
            }
        };
        let output = self
            .outputs
            .get(&selected)
            .ok_or("selected output is incomplete")?;
        if !(output.mode && output.scale && output.transform.is_some() && output.done) {
            return Err("selected output is incomplete".into());
        }
        if output.transform != Some(Transform::Normal) {
            return Err("selected output transform is not Normal".into());
        }
        self.manager_selected = true;
        self.selected_seat = self.seats.iter().next().copied();
        self.selected_output = Some(selected);
        Ok(())
    }
}

struct WaylandClock(std::time::Instant);

impl Clock for WaylandClock {
    fn next_ms(&mut self) -> u32 {
        self.0.elapsed().as_millis() as u32
    }
}

/// Inactive synchronous owner for the selected output-bound virtual pointer.
pub struct WaylandPointerBackend {
    substrate: protocol_substrate::ProtocolSubstrate,
    started: std::time::Instant,
}

impl WaylandPointerBackend {
    pub fn connect(connector: &str) -> Result<Self> {
        Ok(Self {
            substrate: protocol_substrate::ProtocolSubstrate::connect(connector)?,
            started: std::time::Instant::now(),
        })
    }

    pub fn close(&mut self) -> Result<()> {
        self.substrate.close()
    }
}

impl ClickExecutor for WaylandPointerBackend {
    fn click(&mut self, click: &PlannedClick) -> Result<()> {
        ClickTransaction::new(&mut self.substrate, WaylandClock(self.started)).execute(click)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
    fn protocol_substrate_owns_an_adapter_event_queue_without_self_references() {
        protocol_substrate::ProtocolSubstrate::assert_owns_adapter_event_queue();
    }

    #[test]
    fn protocol_substrate_confines_generated_types_without_a_socket() {
        use wayland_client::protocol::wl_registry::WlRegistry;
        use wayland_protocols_wlr::virtual_pointer::v1::client::zwlr_virtual_pointer_manager_v1::ZwlrVirtualPointerManagerV1;

        assert_eq!(ProtocolSubstrate::MANAGER_VERSION, 2);
        let _: Option<WlRegistry> = None;
        let _: Option<ZwlrVirtualPointerManagerV1> = None;
        ProtocolSubstrate::assert_generated_signatures();
    }

    #[test]
    fn registry_substrate_retains_only_advertised_manager_capability_metadata() {
        use wayland_client::{protocol::wl_seat::WlSeat, Proxy};
        use wayland_protocols_wlr::virtual_pointer::v1::client::zwlr_virtual_pointer_manager_v1::ZwlrVirtualPointerManagerV1;

        let mut state = protocol_substrate::RegistryState::default();
        state.record_global(3, WlSeat::interface().name, 8);
        assert_eq!(state.manager, None);

        state.record_global(9, ZwlrVirtualPointerManagerV1::interface().name, 4);
        assert_eq!(
            state.manager,
            Some(protocol_substrate::ManagerGlobal {
                name: 9,
                advertised_version: 4,
            })
        );
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
        protocol_substrate::ProtocolSubstrate::assert_pointer_request_signatures();
    }

    #[test]
    fn generated_client_fails_before_requests_when_manager_is_removed_before_click() {
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

        command.send(Command::Stop).unwrap();
        assert_eq!(
            received.recv().unwrap(),
            ["motion", "frame", "press", "frame", "release", "frame"]
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
}

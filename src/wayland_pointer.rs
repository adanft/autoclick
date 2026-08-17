use anyhow::{anyhow, bail, Result};

/// Wayland protocol boundary: owns the connection, the event queue and the
/// selected output-bound virtual pointer.
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

    /// Callback-owned selected-object state. Proxy user data is a stable registry ID.
    pub(super) struct AdapterState {
        discovery: super::DiscoveryState,
        manager_global: Option<u32>,
        manager: Option<ZwlrVirtualPointerManagerV1>,
        seats: std::collections::BTreeMap<u32, WlSeat>,
        outputs: std::collections::BTreeMap<u32, WlOutput>,
        lifecycle: SelectedPointerLifecycle,
    }

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

        /// The discovery state machine invalidates a selected output whose
        /// metadata is withdrawn, but the `wl_output` dispatch below has no event
        /// that reaches this yet, so only the lifecycle tests drive it.
        #[allow(dead_code)]
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
                    state.manager = Some(registry.bind(
                        name,
                        version.min(super::REQUIRED_MANAGER_VERSION),
                        qh,
                        (),
                    ));
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
    pub(super) struct ProtocolSubstrate {
        connection: Connection,
        event_queue: EventQueue<AdapterState>,
        state: AdapterState,
        pointer: Option<ZwlrVirtualPointerV1>,
    }

    impl ProtocolSubstrate {
        pub(super) const LEFT_BUTTON: u32 = 0x110;

        pub(super) fn connect(connector: &str) -> anyhow::Result<Self> {
            Self::from_connection(Connection::connect_to_env()?, connector)
        }

        pub(super) fn from_connection(
            connection: Connection,
            connector: &str,
        ) -> anyhow::Result<Self> {
            let event_queue = connection.new_event_queue();
            // The registry proxy is not retained: `wl_registry` has no destroy
            // request and wayland-client implements no `Drop` for proxies, so the
            // bound object and its dispatch registration outlive this handle.
            connection.display().get_registry(&event_queue.handle(), ());
            let mut substrate = Self {
                connection,
                event_queue,
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

        /// Dispatches queued compositor events and validates the selected pointer
        /// once, before a transaction starts queueing requests.
        ///
        /// This is the only roundtrip a transaction pays on the way in; the
        /// requests after it are queued and leave together at `barrier`.
        pub(super) fn begin(&mut self) -> anyhow::Result<()> {
            let (event_queue, state) = (&mut self.event_queue, &mut self.state);
            semantic_request(
                state,
                |state| event_queue.roundtrip(state).map(|_| ()).map_err(Into::into),
                || Ok(()),
            )
        }

        /// Returns the pointer for a request that is only queued, never flushed.
        ///
        /// This runs the same full check as `begin`, discovery half included, and
        /// still costs no compositor roundtrip: only the dispatch that normally
        /// accompanies validation talks to the compositor. Keeping the discovery
        /// half matters most after a failed `barrier`, which is the one moment the
        /// selected seat, output or manager is already known gone — recovery must
        /// refuse to write there rather than queue more requests.
        fn queued_pointer(&mut self) -> anyhow::Result<&ZwlrVirtualPointerV1> {
            self.state.validate().map_err(anyhow::Error::msg)?;
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
            self.queued_pointer()?
                .motion_absolute(time, x, y, width, height);
            Ok(())
        }

        pub(super) fn frame(&mut self) -> anyhow::Result<()> {
            self.queued_pointer()?.frame();
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
            self.queued_pointer()?
                .button(time, Self::LEFT_BUTTON, state);
            Ok(())
        }

        /// Flushes the whole queued transaction in one write, then dispatches to
        /// confirm the compositor did not invalidate the pointer meanwhile.
        pub(super) fn barrier(&mut self) -> anyhow::Result<()> {
            self.queued_pointer()?;
            self.connection.flush()?;
            let (event_queue, state) = (&mut self.event_queue, &mut self.state);
            semantic_barrier(
                state,
                |state| event_queue.roundtrip(state).map(|_| ()).map_err(Into::into),
                || Ok(()),
            )
        }

        /// Best-effort release after a transaction failed past the press.
        ///
        /// Both outcomes are deliberate. When the press itself failed while being
        /// queued, returning early leaves it unwritten, which beats flushing a
        /// press with no release behind it. When `barrier` failed after already
        /// flushing the batch, `queued_pointer` refuses here too if the selected
        /// objects are gone, so no request is written against a dead pointer.
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
    }

    impl super::VirtualPointerPort for &mut ProtocolSubstrate {
        fn begin(&mut self) -> anyhow::Result<()> {
            ProtocolSubstrate::begin(self)
        }

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

/// Lowest `zwlr_virtual_pointer_manager_v1` version carrying the output-bound
/// constructor this backend needs.
const REQUIRED_MANAGER_VERSION: u32 = 2;

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ButtonState {
    Pressed,
    Released,
}

/// Semantic operations the click transaction issues against the virtual pointer.
///
/// `begin` validates once; the requests after it are queued and leave the client
/// in one write at `barrier`.
trait VirtualPointerPort {
    fn begin(&mut self) -> Result<()>;
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

trait Clock {
    fn next_ms(&mut self) -> u32;
}

/// One click as a transaction: move, press, release, each framed, then a barrier.
struct ClickTransaction<P, C> {
    port: P,
    clock: C,
    previous_timestamp: Option<u32>,
}

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
        self.port
            .begin()
            .map_err(|error| anyhow!("virtual-pointer transaction rejected: {error}"))?;
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

    /// Test seam: unwraps the port so a recording double can be inspected after
    /// a transaction. The runtime keeps the port for the next click instead.
    #[allow(dead_code)]
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

#[derive(Clone, Copy, PartialEq, Eq)]
enum Transform {
    Normal,
    Rotated,
}

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
    /// See `AdapterState::output_metadata_lost`: reachable from tests only.
    #[allow(dead_code)]
    OutputMetadataLost(u32),
}

#[derive(Default)]
struct OutputState {
    name: Option<String>,
    mode: bool,
    scale: bool,
    transform: Option<Transform>,
    done: bool,
}

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
        if version < REQUIRED_MANAGER_VERSION {
            return Err(format!(
                "virtual-pointer manager version {version} is below v{REQUIRED_MANAGER_VERSION}"
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

/// Synchronous owner of the selected output-bound virtual pointer.
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

impl Drop for WaylandPointerBackend {
    /// Releases the virtual pointer on shutdown. `close` is idempotent, so an
    /// explicit call before drop stays valid.
    fn drop(&mut self) {
        if let Err(error) = self.close() {
            tracing::warn!(error = %error, "Wayland virtual-pointer cleanup failed during shutdown");
        }
    }
}

impl ClickExecutor for WaylandPointerBackend {
    fn click(&mut self, click: &PlannedClick) -> Result<()> {
        ClickTransaction::new(&mut self.substrate, WaylandClock(self.started)).execute(click)
    }
}

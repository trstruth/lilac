use std::collections::HashMap;

use wayland_client::{
    Connection, Dispatch, Proxy, QueueHandle,
    protocol::{
        wl_compositor::{self, WlCompositor},
        wl_output::{self, WlOutput},
        wl_registry,
        wl_surface::{self, WlSurface},
    },
};

use wayland_protocols::ext::session_lock::v1::client::{
    ext_session_lock_manager_v1::{self, ExtSessionLockManagerV1},
    ext_session_lock_surface_v1::{self, ExtSessionLockSurfaceV1},
    ext_session_lock_v1::{self, ExtSessionLockV1},
};

use anyhow::anyhow;

/// This struct represents the state of our app.
/// This type supports the `dispatch` implementations needed for the below state diagram
///
/// Session lock protocol message flow (client perspective).
///
/// CLIENT                                 COMPOSITOR
///   |                                         |
///   | wl_registry.bind ext_session_lock_manager_v1
///   |---------------------------------------->|
///   |                                         |
///   | ext_session_lock_manager_v1.lock
///   |---------------------------------------->|
///   |                                         |
///   |           (either)                      |
///   |<------------------ ext_session_lock_v1.locked
///   |                                         |
///   |  create wl_surface + per-output:
///   |  ext_session_lock_v1.get_lock_surface
///   |---------------------------------------->|
///   |                                         |
///   |<---------------- ext_session_lock_surface_v1.configure
///   |  (width,height,serial)                  |
///   |                                         |
///   |  attach buffer + wl_surface.commit      |
///   |---------------------------------------->|
///   |                                         |
///   | (repeat configure/commit as needed)     |
///   |                                         |
///   |  ... user authenticates ...             |
///   |  ext_session_lock_v1.unlock_and_destroy |
///   |---------------------------------------->|
///   |                                         |
///   | (client destroys lock surfaces)         |
///   |                                         |
///   |<------------------ ext_session_lock_v1.finished (optional)
///   |                                         |
///   | (or, if lock was denied)                |
///   |<------------------ ext_session_lock_v1.finished
///   |  (no locked was sent)                   |
///   |                                         |
#[derive(Default)]
struct Locker {
    lock_manager: Option<ExtSessionLockManagerV1>,
    lock: Option<ExtSessionLockV1>,
    compositor: Option<WlCompositor>,
    monitors: HashMap<u32, Monitor>,
    state: LockState,
}

#[derive(Default)]
struct Monitor {
    name: u32,
    output: Option<WlOutput>,
    surface: Option<WlSurface>,
    lock_surface: Option<ExtSessionLockSurfaceV1>,
    dimensions: (u32, u32),
}

impl Monitor {
    fn with_name(mut self, n: u32) -> Self {
        self.name = n;
        self
    }

    fn with_output(mut self, o: WlOutput) -> Self {
        self.output = Some(o);
        self
    }

    fn create_surface_and_lock(
        &mut self,
        compositor: &WlCompositor,
        lock: &ExtSessionLockV1,
        qh: &QueueHandle<Locker>,
    ) -> anyhow::Result<()> {
        let wl_surface = compositor.create_surface(qh, ());

        let wl_output = self.output.as_ref().ok_or_else(|| {
            anyhow!(format!(
                "monitor with name {} must have Some(output)",
                self.name
            ))
        })?;

        let lock_surface = lock.get_lock_surface(&wl_surface, wl_output, qh, ());

        self.surface = Some(wl_surface);
        self.lock_surface = Some(lock_surface);
        Ok(())
    }
}

enum LockState {
    // haven’t requested a lock yet
    Idle,
    // lock request sent, waiting for locked or finished
    Waiting,
    // received locked, surfaces should be active
    Locked,
    // received finished, lock denied or unlock succeeded
    Finished,
}

impl Default for LockState {
    fn default() -> Self {
        Self::Idle
    }
}

impl Dispatch<wl_registry::WlRegistry, ()> for Locker {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Locker>,
    ) {
        // When receiving events from the wl_registry, we are only interested in the
        // `global` event, which signals a new available global.
        // When receiving this event, we just print its characteristics in this example.
        if let wl_registry::Event::Global {
            name,
            interface,
            version,
        } = event
        {
            match interface.as_str() {
                "ext_session_lock_manager_v1" => {
                    let version = version.min(ExtSessionLockManagerV1::interface().version);
                    let lock_manager =
                        registry.bind::<ExtSessionLockManagerV1, (), Locker>(name, version, qh, ());
                    state.lock_manager = Some(lock_manager);
                }
                "wl_compositor" => {
                    let version = version.min(WlCompositor::interface().version);
                    let compositor =
                        registry.bind::<WlCompositor, (), Locker>(name, version, qh, ());
                    state.compositor = Some(compositor);
                }
                "wl_output" => {
                    let version = version.min(WlOutput::interface().version);
                    let output = registry.bind::<WlOutput, (), Locker>(name, version, qh, ());
                    let disp = Monitor::default().with_name(name).with_output(output);
                    state.monitors.insert(name, disp);
                }
                _ => return,
            }

            println!("Locker found [{}] {} (v{})", name, interface, version);
        }
    }
}

impl Dispatch<ExtSessionLockManagerV1, ()> for Locker {
    fn event(
        _state: &mut Self,
        _: &ExtSessionLockManagerV1,
        _: ext_session_lock_manager_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Locker>,
    ) {
        println!(
            "received an event from ExtSessionLockManager, but don't know what to do with it..."
        )
    }
}

impl Dispatch<ExtSessionLockV1, ()> for Locker {
    fn event(
        state: &mut Self,
        _: &ExtSessionLockV1,
        event: ext_session_lock_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Locker>,
    ) {
        match event {
            // session successfully locked This client is now responsible for displaying
            // graphics while the session is locked and deciding when to unlock the session.
            //
            // The locked event must not be sent until a new “locked” frame has been presented
            // on all outputs and no security sensitive normal/unlocked content is possibly
            // visible.
            //
            // If this event is sent, making the destroy request is a protocol error, the lock
            // object must be destroyed using the unlock_and_destroy request.
            ext_session_lock_v1::Event::Locked => {
                state.state = LockState::Locked;
            }
            // the session lock object should be destroyed
            //
            // The compositor has decided that the session lock should be destroyed as it will
            // no longer be used by the compositor. Exactly when this event is sent is
            // compositor policy, but it must never be sent more than once for a given session
            // lock object.
            //
            // This might be sent because there is already another ext_session_lock_v1 object
            // held by a client, or the compositor has decided to deny the request to lock the
            // session for some other reason. This might also be sent because the compositor
            // implements some alternative, secure way to authenticate and unlock the session.
            //
            // The finished event should be sent immediately on creation of this object if the
            // compositor decides that the locked event will not be sent.
            //
            // If the locked event is sent on creation of this object the finished event may
            // still be sent at some later time in this object’s lifetime. This is compositor
            // policy.
            //
            // Upon receiving this event, the client should make either the destroy request or
            // the unlock_and_destroy request, depending on whether or not the locked event was
            // received on this object.
            ext_session_lock_v1::Event::Finished => {
                state.state = LockState::Finished;
            }
            _ => println!("unknown event received from ExtSessionLock"),
        }
    }
}

impl Dispatch<WlCompositor, ()> for Locker {
    fn event(
        _state: &mut Self,
        _: &WlCompositor,
        _: wl_compositor::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Locker>,
    ) {
        println!("received an event from WlCompositor, but don't know what to do with it...")
    }
}

impl Dispatch<WlOutput, ()> for Locker {
    fn event(
        _state: &mut Self,
        _: &WlOutput,
        _: wl_output::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Locker>,
    ) {
        println!("received an event from WlCompositor, but don't know what to do with it...")
    }
}

impl Dispatch<WlSurface, ()> for Locker {
    fn event(
        _state: &mut Self,
        _: &WlSurface,
        _: wl_surface::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Locker>,
    ) {
        println!("received an event from WlCompositor, but don't know what to do with it...")
    }
}

impl Dispatch<ExtSessionLockSurfaceV1, ()> for Locker {
    fn event(
        state: &mut Self,
        proxy: &ExtSessionLockSurfaceV1,
        event: ext_session_lock_surface_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Locker>,
    ) {
        match event {
            ext_session_lock_surface_v1::Event::Configure {
                width,
                height,
                serial,
            } => {
                let event_proxy_id = proxy.id();
                for monitor in state.monitors.values_mut() {
                    if let Some(lock_surface) = monitor.lock_surface.as_ref() {
                        if lock_surface.id() != event_proxy_id {
                            continue;
                        }

                        let mut final_width = width;
                        let mut final_height = height;

                        if final_width == 0 {
                            final_width = 1920;
                        }

                        if final_height == 0 {
                            final_height = 1080;
                        }
                        monitor.dimensions = (final_width, final_height);

                        lock_surface.ack_configure(serial);

                        return;
                    }
                }
            }
            _ => println!("unknown event rx'd in extsessionlocksurfacev1 dispatch handler"),
        }
    }
}

// The main function of our program
fn main() -> anyhow::Result<()> {
    // Create a Wayland connection by connecting to the server through the
    // environment-provided configuration.
    let conn = Connection::connect_to_env()?;

    // Retrieve the WlDisplay Wayland object from the connection. This object is
    // the starting point of any Wayland program, from which all other objects will
    // be created.
    let display = conn.display();

    // Create an event queue for our event processing
    let mut event_queue = conn.new_event_queue();
    // And get its handle to associate new objects to it
    let qh = event_queue.handle();

    // Create a wl_registry object by sending the wl_display.get_registry request.
    // This method takes two arguments: a handle to the queue that the newly created
    // wl_registry will be assigned to, and the user-data that should be associated
    // with this registry (here it is () as we don't need user-data).
    let _registry = display.get_registry(&qh, ());

    let mut locker = Locker::default();

    // To actually receive the events, we invoke the `roundtrip` method. This method
    // is special and you will generally only invoke it during the setup of your program:
    // it will block until the server has received and processed all the messages you've
    // sent up to now.
    //
    // In our case, that means it'll block until the server has received our
    // wl_display.get_registry request, and as a reaction has sent us a batch of
    // wl_registry.global events.
    //
    // `roundtrip` will then empty the internal buffer of the queue it has been invoked
    // on, and thus invoke our `Dispatch` implementation, which will search for a
    // `ext_session_lock_manager_v1` interface advertisement, and bind to it.
    event_queue.roundtrip(&mut locker)?;

    if locker.lock_manager.is_none() {
        return Err(anyhow!(
            "could not find a lock manager in the registry advertisement"
        ));
    }

    if locker.compositor.is_none() {
        return Err(anyhow!(
            "could not find a compositor in the registry advertisement"
        ));
    }

    if locker.monitors.is_empty() {
        return Err(anyhow!(
            "could not find any outputs in the registry advertisement"
        ));
    }

    // at this point, we're in a happy initial state, as we've registered all of our globals
    let lock = locker
        .lock_manager
        .as_ref()
        .ok_or_else(|| anyhow!("lock manager cannot be empty when trying to call lock"))?
        .lock(&qh, ());
    let compositor = locker
        .compositor
        .as_ref()
        .ok_or_else(|| anyhow!("compositor must not be None when creating surfaces"))?;

    for monitor in locker.monitors.values_mut() {
        monitor.create_surface_and_lock(compositor, &lock, &qh)?;
    }

    locker.lock = Some(lock);
    locker.state = LockState::Waiting;

    loop {
        event_queue.blocking_dispatch(&mut locker)?;
        match locker.state {
            // break out of our loop
            LockState::Finished => break,
            LockState::Idle => {
                return Err(anyhow!(
                    "illegal state: Lock should not have been idle when entering the loop"
                ));
            }
            LockState::Waiting => {
                println!("waiting on the result of calling lock...")
            }
            LockState::Locked => {}
        }
    }

    Ok(())
}

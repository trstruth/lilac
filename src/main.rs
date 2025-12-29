use std::{
    collections::HashMap,
    fs::OpenOptions,
    io::{ErrorKind, Write},
    os::fd::{AsFd, AsRawFd},
    time::{Duration, Instant, SystemTime},
};

use memfd::{Memfd, MemfdOptions};
use mmap::{MapOption, MemoryMap};
use wayland_client::{
    backend::WaylandError,
    Connection, Dispatch, Proxy, QueueHandle,
    protocol::{
        wl_buffer::{self, WlBuffer},
        wl_compositor::{self, WlCompositor},
        wl_output::{self, WlOutput},
        wl_registry,
        wl_shm::{self, WlShm},
        wl_shm_pool::{self, WlShmPool},
        wl_surface::{self, WlSurface},
    },
};

use wayland_protocols::ext::session_lock::v1::client::{
    ext_session_lock_manager_v1::{self, ExtSessionLockManagerV1},
    ext_session_lock_surface_v1::{self, ExtSessionLockSurfaceV1},
    ext_session_lock_v1::{self, ExtSessionLockV1},
};

use anyhow::anyhow;

fn log_line(args: std::fmt::Arguments) {
    let timestamp = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    if let Ok(mut file) = OpenOptions::new()
        .create(true)
        .append(true)
        .open("lilac.log")
    {
        let _ = writeln!(file, "[{}] {}", timestamp, args);
    }
}

macro_rules! logln {
    ($($arg:tt)*) => {
        log_line(format_args!($($arg)*))
    };
}

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
    shared_memory: Option<WlShm>,
    monitors: HashMap<u32, Monitor>,
    state: LockState,
    auto_unlock_deadline: Option<Instant>,
    auto_unlock_sent: bool,
}

impl Locker {
    fn is_initialized(&self) -> anyhow::Result<()> {
        if self.lock_manager.is_none() {
            return Err(anyhow!(
                "could not find a lock manager in the registry advertisement"
            ));
        }

        if self.compositor.is_none() {
            return Err(anyhow!(
                "could not find a compositor in the registry advertisement"
            ));
        }

        if self.shared_memory.is_none() {
            return Err(anyhow!(
                "could not find shared memory in the registry advertisement"
            ));
        }

        if self.monitors.is_empty() {
            return Err(anyhow!(
                "could not find any outputs in the registry advertisement"
            ));
        }
        Ok(())
    }
}

#[derive(Default)]
struct Monitor {
    name: u32,
    output: Option<WlOutput>,
    surface: Option<WlSurface>,
    lock_surface: Option<ExtSessionLockSurfaceV1>,
    dimensions: (u32, u32),
    buffer_state: Option<BufferState>,
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

    fn commit(&mut self) -> anyhow::Result<bool> {
        let buffer_state = self
            .buffer_state
            .as_mut()
            .ok_or_else(|| anyhow!("buffer state cannot be None"))?;

        let Some(buffer_index) = buffer_state.acquire_free_buffer_index() else {
            return Ok(false);
        };
        let buffer = &buffer_state.buffers[buffer_index].buffer;

        let surface = self
            .surface
            .as_ref()
            .ok_or_else(|| anyhow!("surface cannot be None"))?;

        surface.attach(Some(buffer), 0, 0);
        surface.damage_buffer(
            0,
            0,
            self.dimensions.0.try_into()?,
            self.dimensions.1.try_into()?,
        );
        surface.commit();
        buffer_state.buffers[buffer_index].in_use = true;
        buffer_state.dirty = false;
        Ok(true)
    }
}

#[derive(PartialEq, Eq, Copy, Clone)]
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

#[derive(Copy, Clone)]
struct BufferTag {
    monitor_name: u32,
    index: usize,
}

struct BufferSlot {
    // the total number of bytes this buffer contains
    size: i32,
    // the length of a row
    stride: i32,
    // the fd for the data
    mem_fd: Memfd,
    // access to the actual underlying bytes
    bytes: MemoryMap,
    pool: WlShmPool,
    buffer: WlBuffer,
    // whether or not the compositor is currently reading the shared memory
    in_use: bool,
}

struct BufferState {
    buffers: [BufferSlot; 2],
    // whether or not the contents of the buffer in the memory map have been sent to the compositor
    //   - dirty = true whenever UI state changes (input, configure, timer, etc.), regardless of
    //   buffer usage.
    //
    //   - successful render+commit sets dirty = false.
    //
    //   - if a render was desired but all buffers were in use, leave dirty = true and try again
    //   on the next Release.
    dirty: bool,
    next_index: usize,
}

impl BufferState {
    // the flow is:
    //  1. Create an fd (memfd or a temp file) and set_len(size).
    //  2. mmap the fd to get a writable byte slice.
    //  3. Pass that fd to wl_shm.create_pool(fd, size) to get a wl_shm_pool.
    //  4. Create a wl_buffer from the pool with width/height/stride/format.
    fn new(
        shared_memory: &WlShm,
        qh: &QueueHandle<Locker>,
        name: u32,
        width: i32,
        height: i32,
    ) -> anyhow::Result<Self> {
        let buffer_0 = BufferSlot::new(shared_memory, qh, name, 0, width, height)?;
        let buffer_1 = BufferSlot::new(shared_memory, qh, name, 1, width, height)?;

        Ok(Self {
            buffers: [buffer_0, buffer_1],
            dirty: true,
            next_index: 0,
        })
    }

    fn fill_solid_color(&mut self, color: [u8; 4]) {
        for buffer in &mut self.buffers {
            buffer.fill_solid_color(color);
        }
        self.dirty = true;
    }

    fn acquire_free_buffer_index(&mut self) -> Option<usize> {
        let total = self.buffers.len();
        for offset in 0..total {
            let index = (self.next_index + offset) % total;
            if !self.buffers[index].in_use {
                self.next_index = (index + 1) % total;
                return Some(index);
            }
        }
        None
    }
}

impl BufferSlot {
    fn new(
        shared_memory: &WlShm,
        qh: &QueueHandle<Locker>,
        monitor_name: u32,
        index: usize,
        width: i32,
        height: i32,
    ) -> anyhow::Result<Self> {
        let stride = width * 4;
        let size = stride * height;
        let name = monitor_name.wrapping_mul(2).wrapping_add(index as u32);

        let mem_fd_opts = MemfdOptions::default().allow_sealing(true);
        let mem_fd = mem_fd_opts.create(name.to_string())?;
        mem_fd.as_file().set_len(size as u64)?;
        let c_fd = mem_fd.as_file().as_raw_fd();

        let mmap_opts = vec![
            MapOption::MapReadable,
            MapOption::MapWritable,
            MapOption::MapFd(c_fd),
            MapOption::MapNonStandardFlags(libc::MAP_SHARED),
        ];

        let bytes = MemoryMap::new(size as usize, mmap_opts.as_slice())?;

        let pool = shared_memory.create_pool(mem_fd.as_file().as_fd(), size, qh, ());

        let tag = BufferTag {
            monitor_name,
            index,
        };
        let buffer = pool.create_buffer(
            0,
            width,
            height,
            stride,
            wl_shm::Format::Argb8888,
            qh,
            tag,
        );

        Ok(Self {
            size,
            stride,
            mem_fd,
            bytes,
            pool,
            buffer,
            in_use: false,
        })
    }

    fn fill_solid_color(&mut self, color: [u8; 4]) {
        let len = self.size as usize;
        let ptr = self.bytes.data() as *mut u8;
        let data = unsafe { std::slice::from_raw_parts_mut(ptr, len) };

        for px in data.chunks_exact_mut(4) {
            px.copy_from_slice(&color);
        }
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
                "wl_shm" => {
                    let version = version.min(WlShm::interface().version);
                    let shared_memory = registry.bind::<WlShm, (), Locker>(name, version, qh, ());
                    state.shared_memory = Some(shared_memory);
                }
                "wl_output" => {
                    let version = version.min(WlOutput::interface().version);
                    let output = registry.bind::<WlOutput, (), Locker>(name, version, qh, ());
                    let disp = Monitor::default().with_name(name).with_output(output);
                    state.monitors.insert(name, disp);
                }
                _ => return,
            }

            logln!("Locker found [{}] {} (v{})", name, interface, version);
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
        logln!(
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
                logln!("received ext_session_lock_v1::Locked");
                state.state = LockState::Locked;
                state.auto_unlock_deadline = Some(Instant::now() + Duration::from_secs(5));
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
                logln!("received ext_session_lock_v1::Finished");
                state.state = LockState::Finished;
            }
            _ => logln!("unknown event received from ExtSessionLock"),
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
        logln!("received an event from WlCompositor, but don't know what to do with it...")
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
        logln!("received an event from WlOutput, but don't know what to do with it...")
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
        logln!("received an event from WlSurface, but don't know what to do with it...")
    }
}

impl Dispatch<WlShm, ()> for Locker {
    fn event(
        _state: &mut Self,
        _: &WlShm,
        _: wl_shm::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Locker>,
    ) {
        logln!("received an event from WlShm, but don't know what to do with it...")
    }
}

impl Dispatch<WlShmPool, ()> for Locker {
    fn event(
        _state: &mut Self,
        _: &WlShmPool,
        _: wl_shm_pool::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Locker>,
    ) {
        logln!("received an event from WlShmPool, but don't know what to do with it...")
    }
}

impl Dispatch<WlBuffer, BufferTag> for Locker {
    fn event(
        state: &mut Self,
        _: &WlBuffer,
        event: wl_buffer::Event,
        tag: &BufferTag,
        _: &Connection,
        _: &QueueHandle<Locker>,
    ) {
        match event {
            wl_buffer::Event::Release => {
                logln!("received a Release event for WlBuffer");
                let Some(monitor) = state.monitors.get_mut(&tag.monitor_name) else {
                    return;
                };
                let Some(buffer_state) = monitor.buffer_state.as_mut() else {
                    return;
                };

                if tag.index < buffer_state.buffers.len() {
                    buffer_state.buffers[tag.index].in_use = false;
                }
            }
            _ => logln!("received an event from WlBuffer, but don't know what to do with it..."),
        };
    }
}

impl Dispatch<ExtSessionLockSurfaceV1, ()> for Locker {
    fn event(
        state: &mut Self,
        proxy: &ExtSessionLockSurfaceV1,
        event: ext_session_lock_surface_v1::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Locker>,
    ) {
        match event {
            ext_session_lock_surface_v1::Event::Configure {
                width,
                height,
                serial,
            } => {
                let event_proxy_id = proxy.id();
                for (name, monitor) in state.monitors.iter_mut() {
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

                        let shm = &state.shared_memory.as_ref().unwrap();

                        let buffer_state = BufferState::new(
                            shm,
                            qh,
                            *name,
                            final_width.try_into().unwrap(),
                            final_height.try_into().unwrap(),
                        )
                        .unwrap();

                        let mut buffer_state = buffer_state;
                        let blue = 0xFF0000FFu32.to_ne_bytes();
                        buffer_state.fill_solid_color(blue);
                        monitor.buffer_state = Some(buffer_state);
                        match monitor.commit() {
                            Ok(true) => {}
                            Ok(false) => {
                                logln!("all buffers were in use after configure");
                            }
                            Err(err) => {
                                logln!("commit failed after configure: {err}");
                            }
                        }
                    }
                }
            }
            _ => logln!("unknown event rx'd in extsessionlocksurfacev1 dispatch handler"),
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

    locker.is_initialized()?;

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
        conn.flush()?;
        if let Some(guard) = event_queue.prepare_read() {
            match guard.read() {
                Ok(_) => {}
                Err(WaylandError::Io(err)) if err.kind() == ErrorKind::WouldBlock => {}
                Err(err) => return Err(err.into()),
            }
        }

        let dispatched = event_queue.dispatch_pending(&mut locker)?;

        for monitor in locker.monitors.values_mut() {
            let is_dirty = monitor
                .buffer_state
                .as_ref()
                .map(|bs| bs.dirty)
                .unwrap_or(false);

            if is_dirty {
                let committed = monitor.commit()?;
                if !committed {
                    logln!("all buffers were in use, will try to commit on a later event")
                }
            }
        }

        match locker.state {
            // break out of our loop
            LockState::Finished => break,
            LockState::Idle => {
                return Err(anyhow!(
                    "illegal state: Lock should not have been idle when entering the loop"
                ));
            }
            LockState::Waiting => {}
            LockState::Locked => {
                if locker.auto_unlock_sent {
                    continue;
                }
                if let Some(deadline) = locker.auto_unlock_deadline {
                    if Instant::now() >= deadline {
                        if let Some(lock) = locker.lock.as_ref() {
                            lock.unlock_and_destroy();
                            locker.auto_unlock_sent = true;
                        }
                    }
                }
            }
        }

        if dispatched == 0 {
            let mut sleep_for = Duration::from_millis(16);
            if let Some(deadline) = locker.auto_unlock_deadline {
                let now = Instant::now();
                if deadline > now {
                    sleep_for = sleep_for.min(deadline - now);
                } else {
                    sleep_for = Duration::from_millis(0);
                }
            }
            if sleep_for > Duration::from_millis(0) {
                std::thread::sleep(sleep_for);
            }
        }
    }

    Ok(())
}

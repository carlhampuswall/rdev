use crate::linux::common::Display;
use crate::linux::keyboard::Keyboard;
use crate::rdev::{Button, Event, EventType, GrabError, Key, KeyboardState};
use epoll::ControlOptions::{EPOLL_CTL_ADD, EPOLL_CTL_DEL};
use evdev_rs::{
    enums::{EventCode, EV_KEY, EV_REL},
    Device, InputEvent, UInputDevice,
};
use inotify::{Inotify, WatchMask};
use std::ffi::{OsStr, OsString};
use std::fs::{read_dir, File};
use std::io;
use std::os::unix::{
    ffi::OsStrExt,
    fs::FileTypeExt,
    io::{AsRawFd, IntoRawFd, RawFd},
};
use std::path::Path;
use std::time::SystemTime;

// TODO The x, y coordinates are currently wrong !! Is there mouse acceleration
// to take into account ??

macro_rules! convert_keys {
    ($($ev_key:ident, $rdev_key:ident),*) => {
        //TODO: make const when rust lang issue #49146 is fixed
        #[allow(unreachable_patterns)]
        fn evdev_key_to_rdev_key(key: &EV_KEY) -> Option<Key> {
            match key {
                $(
                    EV_KEY::$ev_key => Some(Key::$rdev_key),
                )*
                _ => None,
            }
        }

        // //TODO: make const when rust lang issue #49146 is fixed
        // fn rdev_key_to_evdev_key(key: &Key) -> Option<EV_KEY> {
        //     match key {
        //         $(
        //             Key::$rdev_key => Some(EV_KEY::$ev_key),
        //         )*
        //         _ => None
        //     }
        // }
    };
}

macro_rules! convert_buttons {
    ($($ev_key:ident, $rdev_key:ident),*) => {
        //TODO: make const when rust lang issue #49146 is fixed
        fn evdev_key_to_rdev_button(key: &EV_KEY) -> Option<Button> {
            match key {
                $(
                    EV_KEY::$ev_key => Some(Button::$rdev_key),
                )*
                _ => None,
            }
        }

        // //TODO: make const when rust lang issue #49146 is fixed
        // fn rdev_button_to_evdev_key(event: &Button) -> Option<EV_KEY> {
        //     match event {
        //         $(
        //             Button::$rdev_key => Some(EV_KEY::$ev_key),
        //         )*
        //         _ => None
        //     }
        // }
    };
}

#[rustfmt::skip]
convert_buttons!(
    BTN_LEFT, Left,
    BTN_RIGHT, Right,
    BTN_MIDDLE, Middle
);

//TODO: IntlBackslash, kpDelete
#[rustfmt::skip]
convert_keys!(
    KEY_ESC, Escape,
    KEY_1, Num1,
    KEY_2, Num2,
    KEY_3, Num3,
    KEY_4, Num4,
    KEY_5, Num5,
    KEY_6, Num6,
    KEY_7, Num7,
    KEY_8, Num8,
    KEY_9, Num9,
    KEY_0, Num0,
    KEY_MINUS, Minus,
    KEY_EQUAL, Equal,
    KEY_BACKSPACE, Backspace,
    KEY_TAB, Tab,
    KEY_Q, KeyQ,
    KEY_W, KeyW,
    KEY_E, KeyE,
    KEY_R, KeyR,
    KEY_T, KeyT,
    KEY_Y, KeyY,
    KEY_U, KeyU,
    KEY_I, KeyI,
    KEY_O, KeyO,
    KEY_P, KeyP,
    KEY_LEFTBRACE, LeftBracket,
    KEY_RIGHTBRACE, RightBracket,
    KEY_ENTER, Return,
    KEY_LEFTCTRL, ControlLeft,
    KEY_A, KeyA,
    KEY_S, KeyS,
    KEY_D, KeyD,
    KEY_F, KeyF,
    KEY_G, KeyG,
    KEY_H, KeyH,
    KEY_J, KeyJ,
    KEY_K, KeyK,
    KEY_L, KeyL,
    KEY_SEMICOLON, SemiColon,
    KEY_APOSTROPHE, Quote,
    KEY_GRAVE, BackQuote,
    KEY_LEFTSHIFT, ShiftLeft,
    KEY_BACKSLASH, BackSlash,
    KEY_Z, KeyZ,
    KEY_X, KeyX,
    KEY_C, KeyC,
    KEY_V, KeyV,
    KEY_B, KeyB,
    KEY_N, KeyN,
    KEY_M, KeyM,
    KEY_COMMA, Comma,
    KEY_DOT, Dot,
    KEY_SLASH, Slash,
    KEY_RIGHTSHIFT, ShiftRight,
    KEY_KPASTERISK , KpMultiply,
    KEY_LEFTALT, Alt,
    KEY_SPACE, Space,
    KEY_CAPSLOCK, CapsLock,
    KEY_F1, F1,
    KEY_F2, F2,
    KEY_F3, F3,
    KEY_F4, F4,
    KEY_F5, F5,
    KEY_F6, F6,
    KEY_F7, F7,
    KEY_F8, F8,
    KEY_F9, F9,
    KEY_F10, F10,
    KEY_NUMLOCK, NumLock,
    KEY_SCROLLLOCK, ScrollLock,
    KEY_KP7, Kp7,
    KEY_KP8, Kp8,
    KEY_KP9, Kp9,
    KEY_KPMINUS, KpMinus,
    KEY_KP4, Kp4,
    KEY_KP5, Kp5,
    KEY_KP6, Kp6,
    KEY_KPPLUS, KpPlus,
    KEY_KP1, Kp1,
    KEY_KP2, Kp2,
    KEY_KP3, Kp3,
    KEY_KP0, Kp0,
    KEY_F11, F11,
    KEY_F12, F12,
    KEY_KPENTER, KpReturn,
    KEY_RIGHTCTRL, ControlRight,
    KEY_KPSLASH, KpDivide,
    KEY_RIGHTALT, AltGr,
    KEY_HOME , Home,
    KEY_UP, UpArrow,
    KEY_PAGEUP, PageUp,
    KEY_LEFT, LeftArrow,
    KEY_RIGHT, RightArrow,
    KEY_END, End,
    KEY_DOWN, DownArrow,
    KEY_PAGEDOWN, PageDown,
    KEY_INSERT, Insert,
    KEY_DELETE, Delete,
    KEY_PAUSE, Pause,
    KEY_LEFTMETA, MetaLeft,
    KEY_RIGHTMETA, MetaRight,
    KEY_PRINT, PrintScreen,
    // KpDelete behaves like normal Delete most of the time
    KEY_DELETE, KpDelete,
    // Linux doesn't have an IntlBackslash key
    KEY_BACKSLASH, IntlBackslash
);

fn evdev_event_to_rdev_event(
    event: &InputEvent,
    x: &mut f64,
    y: &mut f64,
    w: f64,
    h: f64,
) -> Option<EventType> {
    match &event.event_code {
        EventCode::EV_KEY(key) => {
            if let Some(button) = evdev_key_to_rdev_button(key) {
                // first check if pressed key is a mouse button
                match event.value {
                    0 => Some(EventType::ButtonRelease(button)),
                    _ => Some(EventType::ButtonPress(button)),
                }
            } else if let Some(key) = evdev_key_to_rdev_key(key) {
                // check if pressed key is a keyboard key
                match event.value {
                    0 => Some(EventType::KeyRelease(key)),
                    _ => Some(EventType::KeyPress(key)),
                }
            } else {
                (unicode_info, kbd.keysym())
            }
        } else {
            (None, 0)
        }
    };

    Event {
        event_type,
        time: SystemTime::now(),
        unicode,
        platform_code,
        position_code: code as _,
        usb_hid: 0,
    }
}

fn grab_keys(display: Arc<Mutex<u64>>, grab_window: libc::c_ulong) {
    unsafe {
        let lock = display.lock().unwrap();
        let display = *lock as *mut xlib::Display;
        xlib::XGrabKeyboard(
            display,
            grab_window,
            c_int::from(true),
            GrabModeAsync,
            GrabModeAsync,
            xlib::CurrentTime,
        );
        xlib::XFlush(display);
    }
    thread::sleep(Duration::from_millis(50));
}

fn ungrab_keys(display: Arc<Mutex<u64>>) {
    {
        let lock = display.lock().unwrap();
        let display = *lock as *mut xlib::Display;
        ungrab_keys_(display);
    }
    thread::sleep(Duration::from_millis(50));
}

fn ungrab_keys_(display: *mut xlib::Display) {
    unsafe {
        xlib::XUngrabKeyboard(display, xlib::CurrentTime);
        xlib::XFlush(display);
    }
}

fn start_callback_event_thread(recv: Receiver<GrabEvent>) {
    thread::spawn(move || loop {
        if let Ok(data) = recv.recv() {
            match data {
                GrabEvent::KeyEvent(event) => unsafe {
                    if let Some(callback) = &mut GLOBAL_CALLBACK {
                        callback(event);
                    }
                },
                GrabEvent::Exit => {
                    break;
                }
            }
        }
    });
}

fn start_grab_service() -> Result<(), GrabError> {
    let (tx, rx) = channel::<GrabEvent>();
    *GRAB_KEY_EVENT_SENDER.lock().unwrap() = Some(tx);

    unsafe {
        // to-do: is display pointer in keyboard always valid?
        // KEYBOARD usage is very confusing and error prone.
        KEYBOARD = Keyboard::new();
        if KEYBOARD.is_none() {
            return Err(GrabError::KeyboardError);
        }
    }

    start_grab_thread();
    start_callback_event_thread(rx);
    Ok(())
}

pub fn filter_map_events<F>(mut func: F) -> io::Result<()>
where
    F: FnMut(InputEvent) -> (Option<InputEvent>, GrabStatus),
{
    let (epoll_fd, mut devices, output_devices) = setup_devices()?;
    let mut inotify = setup_inotify(epoll_fd, &devices)?;

    //grab devices
    devices
        .iter_mut()
        .try_for_each(|device| device.grab(evdev_rs::GrabMode::Grab))?;

    // create buffer for epoll to fill
    let mut epoll_buffer = [epoll::Event::new(epoll::Events::empty(), 0); 4];
    let mut inotify_buffer = vec![0_u8; 4096];
    'event_loop: loop {
        let num_events = epoll::wait(epoll_fd, -1, &mut epoll_buffer)?;

        //map and simulate events, dealing with
        'events: for event in &epoll_buffer[0..num_events] {
            // new device file created
            if event.data == INOTIFY_DATA {
                for event in inotify.read_events(&mut inotify_buffer)? {
                    assert!(
                        event.mask.contains(inotify::EventMask::CREATE),
                        "inotify is listening for events other than file creation"
                    );
                    add_device_to_epoll_from_inotify_event(epoll_fd, event, &mut devices)?;
                }
            } else {
                // Input device recieved event
                let device_idx = event.data as usize;
                let device = devices.get(device_idx).unwrap();
                while device.has_event_pending() {
                    //TODO: deal with EV_SYN::SYN_DROPPED
                    let (_, event) = match device.next_event(evdev_rs::ReadFlag::NORMAL) {
                        Ok(event) => event,
                        Err(_) => {
                            let device_fd = device.fd().unwrap().into_raw_fd();
                            let empty_event = epoll::Event::new(epoll::Events::empty(), 0);
                            epoll::ctl(epoll_fd, EPOLL_CTL_DEL, device_fd, empty_event)?;
                            continue 'events;
                        }
                    };
                    let (event, grab_status) = func(event);

                    if let (Some(event), Some(out_device)) = (event, output_devices.get(device_idx))
                    {
                        out_device.write_event(&event)?;
                    }
                    if grab_status == GrabStatus::Stop {
                        break 'event_loop;
                    }
                }
            }
        }
    }

    for device in devices.iter_mut() {
        //ungrab devices, ignore errors
        device.grab(evdev_rs::GrabMode::Ungrab).ok();
    }

    epoll::close(epoll_fd)?;
    Ok(())
}

static DEV_PATH: &str = "/dev/input";
const INOTIFY_DATA: u64 = u64::max_value();
const EPOLLIN: epoll::Events = epoll::Events::EPOLLIN;

/// Whether to continue grabbing events or to stop
/// Used in `filter_map_events` (and others)
#[derive(Debug, Eq, PartialEq, Hash)]
pub enum GrabStatus {
    /// Stop grabbing
    Continue,
    /// ungrab events
    Stop,
}

fn get_device_files<T>(path: T) -> io::Result<Vec<File>>
where
    T: AsRef<Path>,
{
    let mut res = Vec::new();
    for entry in read_dir(path)? {
        let entry = entry?;
        // /dev/input files are character devices
        if !entry.file_type()?.is_char_device() {
            continue;
        }

        let path = entry.path();
        let file_name_bytes = match path.file_name() {
            Some(file_name) => file_name.as_bytes(),
            None => continue, // file_name was "..", should be impossible
        };
        // skip filenames matching "mouse.* or mice".
        // these files don't play nice with libevdev, not sure why
        // see: https://askubuntu.com/questions/1043832/difference-between-dev-input-mouse0-and-dev-input-mice
        if file_name_bytes == OsStr::new("mice").as_bytes()
            || file_name_bytes
                .get(0..=1)
                .map(|s| s == OsStr::new("js").as_bytes())
                .unwrap_or(false)
            || file_name_bytes
                .get(0..=4)
                .map(|s| s == OsStr::new("mouse").as_bytes())
                .unwrap_or(false)
        {
            continue;
        }
    }
    thread::sleep(Duration::from_millis(50));
}

#[inline]
pub fn enable_grab() {
    send_grab_control(GrabControl::Grab);
}

#[inline]
pub fn disable_grab() {
    send_grab_control(GrabControl::UnGrab);
}

#[inline]
pub fn is_grabbed() -> bool {
    unsafe { IS_GRABBING }
}

pub fn start_grab_listen<T>(callback: T) -> Result<(), GrabError>
where
    T: FnMut(Event) -> Option<Event> + 'static,
{
    if is_grabbed() {
        return Ok(());
    }

    unsafe {
        IS_GRABBING = true;
        GLOBAL_CALLBACK = Some(Box::new(callback));
    }

    start_grab_service()?;
    thread::sleep(Duration::from_millis(100));
    Ok(())
}

/// Returns tuple of epoll_fd, all devices, and uinput devices, where
/// uinputdevices is the same length as devices, and each uinput device is
/// a libevdev copy of its corresponding device.The epoll_fd is level-triggered
/// on any available data in the original devices.
fn setup_devices() -> io::Result<(RawFd, Vec<Device>, Vec<UInputDevice>)> {
    let device_files = get_device_files(DEV_PATH)?;
    let epoll_fd = epoll_watch_all(device_files.iter())?;
    let devices = device_files
        .into_iter()
        .map(Device::new_from_fd)
        .collect::<io::Result<Vec<Device>>>()?;
    let output_devices = devices
        .iter()
        .map(UInputDevice::create_from_device)
        .collect::<io::Result<Vec<UInputDevice>>>()?;
    Ok((epoll_fd, devices, output_devices))
}

/// Creates an inotify instance looking at /dev/input and adds it to an epoll instance.
/// Ensures devices isnt too long, which would make the epoll data ambigious.
fn setup_inotify(epoll_fd: RawFd, devices: &[Device]) -> io::Result<Inotify> {
    //Ensure there is space for inotify at last epoll index.
    if devices.len() as u64 >= INOTIFY_DATA {
        eprintln!("number of devices: {}", devices.len());
        return Err(io::Error::new(
            io::ErrorKind::Other,
            "too many device files!",
        ));
    }
    send_grab_control(GrabControl::Exit);
}

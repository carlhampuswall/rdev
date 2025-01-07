use crate::rdev::{Event, EventType, ListenError};
use crate::windows::common::{convert, set_key_hook, set_mouse_hook, HookError, HOOK, KEYBOARD};
use crate::{
    rdev::{Event, ListenError},
    windows::common::{convert, get_scan_code, set_key_hook, set_mouse_hook, HookError},
};
use std::os::raw::c_int;
use std::ptr::null_mut;
use std::sync::mpsc;
use std::time::SystemTime;
use std::{os::raw::c_int, ptr::null_mut, time::SystemTime};
use winapi::shared::minwindef::{LPARAM, LRESULT, WPARAM};
use winapi::um::winuser::{CallNextHookEx, PeekMessageA, HC_ACTION};
use winapi::{
    shared::{
        basetsd::ULONG_PTR,
        minwindef::{LPARAM, LRESULT, WPARAM},
    },
    um::winuser::{CallNextHookEx, GetMessageA, HC_ACTION, PKBDLLHOOKSTRUCT, PMOUSEHOOKSTRUCT},
};

static mut GLOBAL_CALLBACK: Option<Box<dyn FnMut(Event)>> = None;

impl From<HookError> for ListenError {
    fn from(error: HookError) -> Self {
        match error {
            HookError::Mouse(code) => ListenError::MouseHookError(code),
            HookError::Key(code) => ListenError::KeyHookError(code),
        }
    }
}

unsafe fn raw_callback(
    code: c_int,
    param: WPARAM,
    lpdata: LPARAM,
    f_get_extra_data: impl FnOnce(isize) -> ULONG_PTR,
) -> LRESULT {
    if code == HC_ACTION {
        let (opt, code) = convert(param, lpdata);
        if let Some(event_type) = opt {
            let event = Event {
                event_type,
                time: SystemTime::now(),
                unicode: None,
                platform_code: code as _,
                position_code: get_scan_code(lpdata),
                usb_hid: 0,
                extra_data: f_get_extra_data(lpdata),
            };
            if let Some(ref mut callback) = GLOBAL_CALLBACK {
                callback(event);
            }
        }
    }
    CallNextHookEx(null_mut(), code, param, lpdata)
}

unsafe extern "system" fn raw_callback_mouse(code: i32, param: usize, lpdata: isize) -> isize {
    raw_callback(code, param, lpdata, |data: isize| unsafe {
        (*(data as PMOUSEHOOKSTRUCT)).dwExtraInfo
    })
}

unsafe extern "system" fn raw_callback_keyboard(code: i32, param: usize, lpdata: isize) -> isize {
    raw_callback(code, param, lpdata, |data: isize| unsafe {
        (*(data as PKBDLLHOOKSTRUCT)).dwExtraInfo
    })
}

pub fn listen<T>(callback: T) -> Result<(), ListenError>
where
    T: FnMut(Event) + 'static,
{
    unsafe {
        GLOBAL_CALLBACK = Some(Box::new(callback));
        set_key_hook(raw_callback_keyboard)?;
        if !crate::keyboard_only() {
            set_mouse_hook(raw_callback_mouse)?;
        }

        let (sender, receiver) = mpsc::channel();
        STOP_LOOP = Some(Box::new(move || {
            sender.send(true).unwrap();
        }));
        loop {
            if let Ok(stop_listen) = receiver.try_recv() {
                if stop_listen {
                    break;
                }
            }
            PeekMessageA(null_mut(), null_mut(), 0, 0, 0);
        }
    }
    Ok(())
}

pub fn stop_listen() {
    unsafe {
        if let Some(stop_loop) = STOP_LOOP.as_ref() {
            stop_loop();
            STOP_LOOP = None;
        }
    }
}

type DynFn = dyn Fn() + 'static;
pub static mut STOP_LOOP: Option<Box<DynFn>> = None;

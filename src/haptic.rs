use std::ffi::{c_char, c_void};
use std::time::{Duration, Instant};

#[link(name = "AppKit", kind = "framework")]
unsafe extern "C" {
    fn objc_getClass(name: *const c_char) -> *mut c_void;
    fn sel_registerName(name: *const c_char) -> *mut c_void;
    fn objc_msgSend();
}

type SendId = unsafe extern "C" fn(*mut c_void, *mut c_void) -> *mut c_void;
type SendVoid2 = unsafe extern "C" fn(*mut c_void, *mut c_void, isize, isize);
type SendVoidInt = unsafe extern "C" fn(*mut c_void, *mut c_void, isize);

unsafe fn init_app() {
    let cls = unsafe { objc_getClass(c"NSApplication".as_ptr()) };
    if cls.is_null() {
        return;
    }
    let sel = unsafe { sel_registerName(c"sharedApplication".as_ptr()) };
    let send: SendId = unsafe { std::mem::transmute(objc_msgSend as *const ()) };
    let app = unsafe { send(cls, sel) };
    if app.is_null() {
        return;
    }
    let sel = unsafe { sel_registerName(c"setActivationPolicy:".as_ptr()) };
    let send2: SendVoidInt = unsafe { std::mem::transmute(objc_msgSend as *const ()) };
    // NSApplicationActivationPolicyAccessory: no Dock icon, but AppKit is initialized
    unsafe { send2(app, sel, 1) };
}

unsafe fn perform_feedback() {
    let cls = unsafe { objc_getClass(c"NSHapticFeedbackManager".as_ptr()) };
    if cls.is_null() {
        return;
    }
    let sel = unsafe { sel_registerName(c"defaultPerformer".as_ptr()) };
    let send: SendId = unsafe { std::mem::transmute(objc_msgSend as *const ()) };
    let performer = unsafe { send(cls, sel) };
    if performer.is_null() {
        return;
    }
    let sel = unsafe { sel_registerName(c"performFeedbackPattern:performanceTime:".as_ptr()) };
    let send2: SendVoid2 = unsafe { std::mem::transmute(objc_msgSend as *const ()) };
    // NSHapticFeedbackPatternAlignment = 1, NSHapticFeedbackPerformanceTimeNow = 1
    unsafe { send2(performer, sel, 1, 1) };
}

pub struct Haptics {
    enabled: bool,
    last: Instant,
}

impl Haptics {
    pub fn new(enabled: bool) -> Self {
        if enabled {
            unsafe { init_app() };
        }
        Self {
            enabled,
            last: Instant::now() - Duration::from_secs(1),
        }
    }

    /// A subtle alignment tick, throttled so fast scrolling does not turn into a buzz.
    pub fn tick(&mut self) {
        if !self.enabled {
            return;
        }
        let now = Instant::now();
        if now.duration_since(self.last) < Duration::from_millis(55) {
            return;
        }
        self.last = now;
        unsafe { perform_feedback() };
    }
}

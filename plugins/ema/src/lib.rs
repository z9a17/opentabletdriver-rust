use otd_plugin_api::{FilterApi, Header, Sample, plugin_name};
use serde::Deserialize;
use std::ffi::c_void;

#[derive(Deserialize)]
#[serde(default, deny_unknown_fields)]
struct Settings {
    alpha: f32,
    reset_ms: u32,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            alpha: 0.35,
            reset_ms: 50,
        }
    }
}

struct Filter {
    alpha: f32,
    reset_ns: u64,
    previous: Option<Sample>,
}

unsafe extern "C" fn create(json: *const u8, len: usize) -> *mut c_void {
    if json.is_null() || len > 65_536 {
        return std::ptr::null_mut();
    }
    let text = unsafe { std::slice::from_raw_parts(json, len) };
    let Ok(settings) = serde_json::from_slice::<Settings>(text) else {
        return std::ptr::null_mut();
    };
    if !settings.alpha.is_finite() || !(0.0..=1.0).contains(&settings.alpha) {
        return std::ptr::null_mut();
    }
    Box::into_raw(Box::new(Filter {
        alpha: settings.alpha,
        reset_ns: u64::from(settings.reset_ms) * 1_000_000,
        previous: None,
    }))
    .cast()
}

unsafe extern "C" fn process(context: *mut c_void, sample: *mut Sample) -> i32 {
    if context.is_null() || sample.is_null() {
        return 1;
    }
    let filter = unsafe { &mut *context.cast::<Filter>() };
    let sample = unsafe { &mut *sample };
    if let Some(previous) = filter.previous
        && sample.time_ns.saturating_sub(previous.time_ns) <= filter.reset_ns
    {
        sample.x = previous.x + filter.alpha * (sample.x - previous.x);
        sample.y = previous.y + filter.alpha * (sample.y - previous.y);
    }
    filter.previous = Some(*sample);
    0
}

unsafe extern "C" fn reset(context: *mut c_void) {
    if !context.is_null() {
        unsafe { &mut *context.cast::<Filter>() }.previous = None;
    }
}

unsafe extern "C" fn destroy(context: *mut c_void) {
    if !context.is_null() {
        drop(unsafe { Box::from_raw(context.cast::<Filter>()) });
    }
}

static API: FilterApi = FilterApi {
    header: Header::V1,
    name: plugin_name("Exponential smoothing"),
    create: Some(create),
    process: Some(process),
    reset: Some(reset),
    destroy: Some(destroy),
};

#[unsafe(no_mangle)]
pub extern "C" fn otd_filter_v1() -> *const FilterApi {
    &API
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn smooths_and_resets_and_rejects_invalid_configuration() {
        unsafe {
            let json = br#"{"alpha":0.5}"#;
            let context = create(json.as_ptr(), json.len());
            assert!(!context.is_null());
            let mut sample = Sample {
                x: 100.0,
                y: 200.0,
                ..Sample::default()
            };
            assert_eq!(process(context, &mut sample), 0);
            sample.x = 200.0;
            sample.y = 400.0;
            sample.time_ns = 1_000_000;
            process(context, &mut sample);
            assert_eq!((sample.x, sample.y), (150.0, 300.0));
            reset(context);
            sample.x = 500.0;
            process(context, &mut sample);
            assert_eq!(sample.x, 500.0);
            sample.x = 800.0;
            sample.time_ns += 51_000_000;
            process(context, &mut sample);
            assert_eq!(sample.x, 800.0);
            destroy(context);
            for json in [br#"{"alpha":2}"#.as_slice(), br#"{"typo":1}"#, b"null"] {
                assert!(create(json.as_ptr(), json.len()).is_null());
            }
        }
    }
}

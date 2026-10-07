//! Synchronous managed graph bridge. Every raw pointer is scoped to one call;
//! no callback borrows the PluginChain whose graph is executing.
use otd_core::reports::*;
use std::ffi::c_void;
use std::io;

const POSITION: u32 = 1;
const TABLET: u32 = 2;
const ERASER: u32 = 4;
const TILT: u32 = 8;
const PROXIMITY: u32 = 16;
const TOOL: u32 = 32;
const AUX: u32 = 64;
const MOUSE: u32 = 128;
const ABSOLUTE: u32 = 256;
const RELATIVE: u32 = 512;
const ABSOLUTE_WHEEL: u32 = 1024;
const RELATIVE_WHEEL: u32 = 2048;
const WHEEL_BUTTONS: u32 = 4096;
const TOUCH: u32 = 8192;
const NATIVE_TIP: u32 = 16384;

pub(super) type CreateGraph = unsafe extern "C" fn(*const GraphNode, u32) -> *mut c_void;
pub(super) type DispatchGraph =
    unsafe extern "C" fn(*mut c_void, *const GraphReport, Callback, *mut c_void) -> i32;
pub(super) type GraphFailure = unsafe extern "C" fn(*mut c_void) -> i32;
pub(super) type DestroyGraph = unsafe extern "C" fn(*mut c_void);
pub(super) type GraphNextTick = unsafe extern "C" fn(*mut c_void) -> i64;
pub(super) type TickGraph = unsafe extern "C" fn(*mut c_void, Callback, *mut c_void) -> i32;
pub type Callback = unsafe extern "C" fn(*mut c_void, u32, u32, *mut GraphReport) -> i32;

#[repr(C)]
pub struct GraphNode {
    pub context: *mut c_void,
    pub index: u32,
    pub stage: u32,
}

/// Keep layout synchronized with compat/OtdCompat/Graph.cs. Arrays are inline
/// implementation capacities, and every incoming count is checked before use.
#[repr(C)]
pub struct GraphReport {
    version: u32,
    size: u32,
    raw: *const u8,
    raw_length: u32,
    kind: u32,
    flags: u32,
    reserved: u32,
    serial: u64,
    pen_bits: u64,
    aux_bits: u64,
    mouse_bits: u64,
    x: f32,
    y: f32,
    tilt_x: f32,
    tilt_y: f32,
    scroll_x: f32,
    scroll_y: f32,
    pressure: u32,
    eraser: u32,
    near: u32,
    distance: u32,
    tool_id: u32,
    tool_type: u32,
    pen_count: u32,
    aux_count: u32,
    mouse_count: u32,
    tip_switch: u32,
    absolute_count: u32,
    absolute_present: u32,
    relative_count: u32,
    wheel_count: u32,
    touch_count: u32,
    touch_present: u32,
    absolute_values: [u32; MAX_ANALOG_CHANNELS],
    relative_values: [i32; MAX_ANALOG_CHANNELS],
    wheel_bits: [u64; MAX_WHEELS],
    wheel_counts: [u32; MAX_WHEELS],
    touch_ids: [u32; MAX_TOUCH_POINTS],
    touch_xy: [f32; MAX_TOUCH_POINTS * 2],
}

fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}
fn capacity(error: ReportError) -> io::Error {
    io::Error::other(format!("managed report: {error:?}"))
}

impl GraphReport {
    /// Operation 6 has a tagged service projection, rather than report shape.
    pub fn managed_command(&self) -> io::Result<otd_core::plugins::ManagedCommand> {
        if self.version != 2 || self.size != std::mem::size_of::<Self>() as u32 || self.kind > 4 { return Err(invalid("invalid managed service command")); }
        Ok(otd_core::plugins::ManagedCommand { kind: self.kind, owner: self.reserved, x: self.x, y: self.y,
            value: self.pressure, flags: self.flags, tilt: [self.tilt_x, self.tilt_y] })
    }
    pub fn new(kind: ReportKind, values: &ReportValues, raw: &[u8]) -> io::Result<Self> {
        if raw.len() > usize::from(u16::MAX) {
            return Err(invalid("native graph packet exceeds transport capacity"));
        }
        // All fields permit an all-zero representation; pointers start null and
        // are assigned the caller's borrowed raw view below.
        let mut frame: Self = unsafe { std::mem::zeroed() };
        frame.version = 2;
        frame.size = std::mem::size_of::<Self>() as u32;
        frame.raw = raw.as_ptr();
        frame.raw_length = raw.len() as u32;
        frame.kind = u32::from(kind == ReportKind::OutOfRange);
        if let Some([x, y]) = values.position {
            frame.flags |= POSITION;
            frame.x = x;
            frame.y = y;
        }
        match (values.pressure, values.pen_buttons) {
            (Some(pressure), Some(buttons)) if values.position.is_some() => {
                frame.flags |= TABLET;
                frame.pressure = pressure;
                frame.pen_bits = buttons.bits();
                frame.pen_count = buttons.len() as u32;
            }
            (None, None) => {}
            _ => {
                return Err(invalid(
                    "ITabletReport requires position, pressure and pen buttons",
                ));
            }
        }
        if let Some(eraser) = values.eraser {
            frame.flags |= ERASER;
            frame.eraser = u32::from(eraser);
        }
        if let Some([x, y]) = values.tilt {
            frame.flags |= TILT;
            frame.tilt_x = x;
            frame.tilt_y = y;
        }
        // A partial transport extension is not the full upstream interface.
        if let (Some(near), Some(distance)) = (values.near_proximity, values.hover_distance) {
            frame.flags |= PROXIMITY;
            frame.near = u32::from(near);
            frame.distance = distance;
        }
        if let Some(tool) = values.tool {
            frame.flags |= TOOL;
            frame.serial = tool.serial;
            frame.tool_id = tool.raw_tool_id;
            frame.tool_type = u32::from(tool.tool == ToolType::Eraser);
        }
        if let Some(buttons) = values.aux_buttons {
            frame.flags |= AUX;
            frame.aux_bits = buttons.bits();
            frame.aux_count = buttons.len() as u32;
        }
        match (values.mouse_buttons, values.mouse_scroll) {
            (Some(buttons), Some([x, y])) if values.position.is_some() => {
                frame.flags |= MOUSE;
                frame.mouse_bits = buttons.bits();
                frame.mouse_count = buttons.len() as u32;
                frame.scroll_x = x;
                frame.scroll_y = y;
            }
            (None, None) => {}
            _ => {
                return Err(invalid(
                    "IMouseReport requires position, buttons and scroll",
                ));
            }
        }
        if let Some(analog) = values.absolute_analog {
            frame.flags |= ABSOLUTE;
            if analog.kind == AnalogKind::Wheel {
                frame.flags |= ABSOLUTE_WHEEL;
            }
            frame.absolute_count = analog.positions.len() as u32;
            for (i, value) in analog.positions.as_slice().iter().enumerate() {
                if let Some(value) = value {
                    frame.absolute_present |= 1 << i;
                    frame.absolute_values[i] = *value;
                }
            }
        }
        if let Some(analog) = values.relative_analog {
            frame.flags |= RELATIVE;
            if analog.kind == AnalogKind::Wheel {
                frame.flags |= RELATIVE_WHEEL;
            }
            frame.relative_count = analog.deltas.len() as u32;
            frame.relative_values[..analog.deltas.len()].copy_from_slice(analog.deltas.as_slice());
        }
        if let Some(wheels) = values.wheel_buttons {
            frame.flags |= WHEEL_BUTTONS;
            frame.wheel_count = wheels.len() as u32;
            for (i, buttons) in wheels.as_slice().iter().enumerate() {
                frame.wheel_bits[i] = buttons.bits();
                frame.wheel_counts[i] = buttons.len() as u32;
            }
        }
        if let Some(touches) = values.touches {
            frame.flags |= TOUCH;
            frame.touch_count = touches.len() as u32;
            for (i, point) in touches.as_slice().iter().enumerate() {
                if let Some(point) = point {
                    frame.touch_present |= 1 << i;
                    frame.touch_ids[i] = u32::from(point.id);
                    frame.touch_xy[i * 2..i * 2 + 2].copy_from_slice(&point.position);
                }
            }
        }
        if let Some(tip) = values.tip_switch {
            frame.flags |= NATIVE_TIP;
            frame.tip_switch = u32::from(tip);
        }
        Ok(frame)
    }

    pub fn decode(&self) -> io::Result<(ReportKind, ReportValues)> {
        if self.version != 2
            || self.size != std::mem::size_of::<Self>() as u32
            || self.flags & !32767 != 0
            || self.kind > 1
            || self.absolute_count as usize > MAX_ANALOG_CHANNELS
            || self.relative_count as usize > MAX_ANALOG_CHANNELS
            || self.wheel_count as usize > MAX_WHEELS
            || self.touch_count as usize > MAX_TOUCH_POINTS
        {
            return Err(invalid("invalid managed graph report layout or capacities"));
        }
        let has = |flag| self.flags & flag != 0;
        let mut values = ReportValues::default();
        if has(POSITION) {
            values.position = Some([self.x, self.y]);
        }
        if has(TABLET) {
            if !has(POSITION) {
                return Err(invalid("tablet report has no position"));
            }
            values.pressure = Some(self.pressure);
            values.pen_buttons =
                Some(Buttons::from_bits(self.pen_bits, self.pen_count as usize).map_err(capacity)?);
        }
        if has(ERASER) {
            values.eraser = Some(self.eraser != 0);
        }
        if has(TILT) {
            values.tilt = Some([self.tilt_x, self.tilt_y]);
        }
        if has(PROXIMITY) {
            values.near_proximity = Some(self.near != 0);
            values.hover_distance = Some(self.distance);
        }
        if has(TOOL) {
            let tool = match self.tool_type {
                0 => ToolType::Pen,
                1 => ToolType::Eraser,
                _ => return Err(invalid("unknown managed report tool type")),
            };
            values.tool = Some(ToolIdentity {
                serial: self.serial,
                raw_tool_id: self.tool_id,
                tool,
            });
        }
        if has(AUX) {
            values.aux_buttons =
                Some(Buttons::from_bits(self.aux_bits, self.aux_count as usize).map_err(capacity)?);
        }
        if has(MOUSE) {
            if !has(POSITION) {
                return Err(invalid("mouse report has no position"));
            }
            values.mouse_buttons = Some(
                Buttons::from_bits(self.mouse_bits, self.mouse_count as usize).map_err(capacity)?,
            );
            values.mouse_scroll = Some([self.scroll_x, self.scroll_y]);
        }
        if has(ABSOLUTE) {
            let mut positions = AbsoluteAnalog::default();
            for i in 0..self.absolute_count as usize {
                positions
                    .push(
                        (self.absolute_present & (1 << i) != 0).then_some(self.absolute_values[i]),
                    )
                    .map_err(capacity)?;
            }
            values.absolute_analog = Some(AbsoluteAnalogReport {
                kind: if has(ABSOLUTE_WHEEL) {
                    AnalogKind::Wheel
                } else {
                    AnalogKind::Generic
                },
                positions,
            });
        }
        if has(RELATIVE) {
            values.relative_analog = Some(RelativeAnalogReport {
                kind: if has(RELATIVE_WHEEL) {
                    AnalogKind::Wheel
                } else {
                    AnalogKind::Generic
                },
                deltas: RelativeAnalog::from_slice(
                    &self.relative_values[..self.relative_count as usize],
                )
                .map_err(capacity)?,
            });
        }
        if has(WHEEL_BUTTONS) {
            let mut wheels = WheelButtons::default();
            for i in 0..self.wheel_count as usize {
                wheels
                    .push(
                        Buttons::from_bits(self.wheel_bits[i], self.wheel_counts[i] as usize)
                            .map_err(capacity)?,
                    )
                    .map_err(capacity)?;
            }
            values.wheel_buttons = Some(wheels);
        }
        if has(TOUCH) {
            let mut touches = Touches::default();
            for i in 0..self.touch_count as usize {
                let point = if self.touch_present & (1 << i) != 0 {
                    Some(TouchPoint {
                        id: u8::try_from(self.touch_ids[i])
                            .map_err(|_| invalid("touch ID exceeds upstream byte range"))?,
                        position: [self.touch_xy[i * 2], self.touch_xy[i * 2 + 1]],
                    })
                } else {
                    None
                };
                touches.push(point).map_err(capacity)?;
            }
            values.touches = Some(touches);
        }
        if has(NATIVE_TIP) {
            values.tip_switch = Some(self.tip_switch != 0);
        }
        Ok((
            if self.kind == 1 {
                ReportKind::OutOfRange
            } else {
                ReportKind::Data
            },
            values,
        ))
    }

    /// # Safety
    /// Caller must ensure the bridge's fixed raw-array pin remains live for the
    /// entire returned borrow. Never store this slice beyond the callback.
    pub unsafe fn raw(&self) -> io::Result<&[u8]> {
        if self.raw_length == 0 {
            return Ok(&[]);
        }
        if self.raw.is_null() || self.raw_length as usize > isize::MAX as usize {
            return Err(invalid("invalid managed raw view"));
        }
        Ok(unsafe { std::slice::from_raw_parts(self.raw, self.raw_length as usize) })
    }

    pub fn set_position(&mut self, values: &ReportValues) {
        if let Some([x, y]) = values.position {
            self.x = x;
            self.y = y;
        }
    }
}

pub struct Graph {
    context: *mut c_void,
    timer_capability: bool,
}
impl Graph {
    pub fn new(nodes: &[GraphNode]) -> Result<Self, String> {
        if nodes.len() > 32 {
            return Err("The synchronous graph supports at most 32 filters".into());
        }
        let context =
            unsafe { (super::bridge()?.create_graph)(nodes.as_ptr(), nodes.len() as u32) };
        if context.is_null() {
            return Err(super::last_error());
        }
        // -2 means no injected timers exist. -1 merely means stopped, and
        // must still be polled after a plugin starts its timer in Consume.
        let timer_capability = super::bridge()?
            .graph_next_tick
            .is_some_and(|query| unsafe { query(context) } != -2);
        Ok(Self {
            context,
            timer_capability,
        })
    }

    pub fn attach_output(&mut self, output: &super::endpoints::OutputSession) -> Result<(), String> {
        output.attach(self.context)?;
        self.timer_capability = true;
        Ok(())
    }
    /// Time until the next filter timer tick, or `None` without timers.
    pub fn next_tick(&self) -> Option<std::time::Duration> {
        if !self.timer_capability {
            return None;
        }
        let next = super::bridge().ok()?.graph_next_tick?;
        u64::try_from(unsafe { next(self.context) })
            .ok()
            .map(std::time::Duration::from_micros)
    }

    /// Fires due filter timers.
    ///
    /// # Safety
    /// As for `dispatch`: `scope` must be valid for every invocation of
    /// `callback`, which must not unwind or retain frame pointers.
    pub unsafe fn tick(&self, callback: Callback, scope: *mut c_void) -> io::Result<()> {
        let bridge = super::bridge().map_err(io::Error::other)?;
        let Some(tick) = bridge.tick_graph2.or(bridge.tick_graph) else {
            return Ok(());
        };
        if unsafe { tick(self.context, callback, scope) } != 0 {
            return Err(io::Error::other(super::last_error()));
        }
        Ok(())
    }

    /// The scope is live exclusively for this synchronous call. The .NET graph
    /// clears its native callback and scope in a finally block before returning.
    ///
    /// # Safety
    /// The raw slice passed to GraphReport::new must also remain live and
    /// unchanged until this call returns; the bridge copies it on entry.
    /// `scope` must be valid for every invocation of `callback`, which must not
    /// unwind or retain any frame/raw pointers after returning.
    /// When `runs_builtins_in_host` is true, the caller must already have run
    /// the built-in filters on the frame's report.
    pub unsafe fn dispatch(
        &self,
        frame: &GraphReport,
        callback: Callback,
        scope: *mut c_void,
    ) -> io::Result<()> {
        let bridge = super::bridge().map_err(io::Error::other)?;
        let dispatch = bridge.dispatch_graph2.unwrap_or(bridge.dispatch_graph);
        if unsafe { dispatch(self.context, frame, callback, scope) } != 0 {
            return Err(io::Error::other(super::last_error()));
        }
        Ok(())
    }
    /// Whether `dispatch` expects the built-in filters to have run already.
    /// The fused bridge skips the continuation that would run them.
    pub fn runs_builtins_in_host(&self) -> bool {
        super::bridge().is_ok_and(|bridge| bridge.dispatch_graph2.is_some())
    }
    pub fn failed_index(&self) -> Option<usize> {
        let bridge = super::bridge().ok()?;
        usize::try_from(unsafe { (bridge.graph_failure)(self.context) }).ok()
    }
}
impl Drop for Graph {
    fn drop(&mut self) {
        if let Ok(bridge) = super::bridge() {
            unsafe { (bridge.destroy_graph)(self.context) };
        }
    }
}

//! Native port of pinned Desktop MacOSVirtualMouse's click and tablet fields.
//! Shared button ownership chooses actual edges; each event carries the issuing
//! reader's copied attributes. Rust-owned successful dispatch allocates no heap.
use std::{io,ptr,sync::{Mutex,OnceLock},time::{Duration,Instant}};
use otd_core::output::MouseAttributes;
use crate::{ffi,macos::{Owned,event_timestamp}};

const DEVICE_ID:i64=5303613955435230461;
const CAPABILITIES:i64=0x001|0x002|0x004|0x040|0x080|0x100|0x400;

struct Clicks {count:i64,started:Option<Instant>,down:ffi::Point,moved:bool,interval:Duration,last_button:u8}
impl Clicks {
    fn movement(&mut self,point:ffi::Point){let dx=point.x-self.down.x;let dy=point.y-self.down.y;if dx*dx+dy*dy>64.0{self.moved=true;}}
    fn edge(&mut self,held:bool,point:ffi::Point,now:Instant){
        self.movement(point);
        let expired=self.moved||self.started.is_some_and(|started|now.duration_since(started)>self.interval);
        if held {if self.count==0||expired{self.started=Some(now);self.down=point;self.moved=false;self.count=1;}else{self.count=self.count.saturating_add(1);}}
        else if expired{self.count=0;}
    }
}
struct Pointer {source:Owned,attributes:MouseAttributes,clicks:Clicks,buttons:u8,last_proximity:Instant}
// Sole accessor is this module's mutex. CGEvents are not tied to a CFRunLoop.
unsafe impl Send for Pointer {}
static POINTER:OnceLock<Mutex<Option<Pointer>>>=OnceLock::new();
fn with<T>(action:impl FnOnce(&mut Pointer)->io::Result<T>)->io::Result<T>{
    let mut pointer=POINTER.get_or_init(||Mutex::new(None)).lock().map_err(|_|io::Error::other("Shared CoreGraphics pointer poisoned"))?;
    action(pointer.as_mut().ok_or_else(||io::Error::other("Shared CoreGraphics pointer unavailable"))?)
}
pub fn ensure()->io::Result<()>{
    let mut pointer=POINTER.get_or_init(||Mutex::new(None)).lock().map_err(|_|io::Error::other("Shared CoreGraphics pointer poisoned"))?;
    if pointer.is_some(){return Ok(());}
    // Same actual NSEvent system preference as the pinned Desktop implementation.
    let interval=unsafe {let class=ffi::objc_getClass(c"NSEvent".as_ptr());let selector=ffi::sel_registerName(c"doubleClickInterval".as_ptr());
        if class.is_null()||selector.is_null(){return Err(io::Error::other("NSEvent doubleClickInterval unavailable"));}ffi::objc_msgSend_double(class,selector)};
    if !interval.is_finite()||interval<=0.0||interval>60.0{return Err(io::Error::other("Invalid system double-click interval"));}
    let source=Owned(unsafe{ffi::CGEventSourceCreate(-1)});if source.0.is_null(){return Err(io::Error::other("Cannot create shared tablet event source"));}
    *pointer=Some(Pointer{source,attributes:MouseAttributes::default(),clicks:Clicks{count:0,started:None,down:ffi::Point::default(),moved:false,interval:Duration::from_secs_f64(interval),last_button:0},buttons:0,last_proximity:Instant::now()});Ok(())
}
pub fn attributes(attributes:MouseAttributes)->io::Result<()>{with(|pointer|pointer.attributes(attributes))}
pub fn button(button:u8,held:bool,context:Option<otd_platform::input_owner::PointerContext>,flags:u64)->io::Result<()>{with(|pointer|{
    if let Some(attributes)=context.and_then(|context|context.attributes){pointer.attributes(attributes)?;}
    let point=match context.and_then(|context|context.position){Some((x,y))=>ffi::Point{x,y},None=>{let query=Owned(unsafe{ffi::CGEventCreate(ptr::null())});if query.0.is_null(){return Err(io::Error::other("Cannot query mouse binding cursor"));}unsafe{ffi::CGEventGetLocation(query.0)}}};
    let previous=pointer.buttons;let buttons=if held{previous|(1<<button)}else{previous&!(1<<button)};
    let click_point=context.and_then(|context|context.click_position).map_or(point,|(x,y)|ffi::Point{x,y});
    pointer.clicks.edge(held,click_point,Instant::now());pointer.clicks.last_button=button;
    let kind=match(button,held){(0,true)=>1,(0,false)=>2,(1,true)=>3,(1,false)=>4,(_,true)=>25,(_,false)=>26};
    let delta=context.and_then(|context|context.delta).map_or(ffi::Point::default(),|(x,y)|ffi::Point{x,y});
    pointer.post(kind,button,point,delta,previous,buttons,flags)?;pointer.buttons=buttons;Ok(())
})}
pub fn movement(point:ffi::Point,delta:ffi::Point,click_point:ffi::Point,attributes:MouseAttributes,flags:u64)->io::Result<()>{with(|pointer|{
    pointer.attributes(attributes)?;pointer.clicks.movement(click_point);
    let button=pointer.clicks.last_button;let kind=if pointer.buttons&(1<<button)==0{5}else{match button{0=>6,1=>7,_=>27}};
    pointer.post(kind,button,point,delta,pointer.buttons,pointer.buttons,flags)
})}
impl Pointer {
    fn attributes(&mut self,attributes:MouseAttributes)->io::Result<()>{
        if let Some(pressure)=attributes.pressure{if !pressure.is_finite(){return Err(io::Error::new(io::ErrorKind::InvalidData,"Nonfinite pointer pressure"));}self.attributes.pressure=Some(pressure);}
        if let Some(tilt)=attributes.tilt{if tilt.iter().any(|value|!value.is_finite()){return Err(io::Error::new(io::ErrorKind::InvalidData,"Nonfinite pointer tilt"));}self.attributes.tilt=Some(tilt);}
        if let Some(eraser)=attributes.eraser{if self.attributes.eraser!=Some(eraser){self.attributes.eraser=Some(eraser);self.proximity()?;self.last_proximity=Instant::now();}}
        // Original Reset releases owned buttons; setters retain their values.
        Ok(())
    }
    fn proximity_fields(&self,event:ffi::Ref){unsafe{
        ffi::CGEventSetIntegerValueField(event,38,1);ffi::CGEventSetIntegerValueField(event,37,if self.attributes.eraser.unwrap_or(false){3}else{1});
        ffi::CGEventSetIntegerValueField(event,36,CAPABILITIES);ffi::CGEventSetIntegerValueField(event,31,DEVICE_ID);ffi::CGEventSetIntegerValueField(event,33,0x802);
    }}
    fn proximity(&self)->io::Result<()>{let event=Owned(unsafe{ffi::CGEventCreate(self.source.0)});if event.0.is_null(){return Err(io::Error::other("Cannot create tablet proximity event"));}
        unsafe{ffi::CGEventSetType(event.0,24);}self.proximity_fields(event.0);unsafe{ffi::CGEventSetTimestamp(event.0,event_timestamp()?);ffi::CGEventPost(0,event.0);}Ok(())
    }
    fn post(&mut self,kind:u32,button:u8,point:ffi::Point,delta:ffi::Point,previous:u8,buttons:u8,flags:u64)->io::Result<()>{
        // A fresh native event avoids union values leaking when switching
        // tablet-point and tablet-proximity subtypes (pinned PostEvent rule).
        let event=Owned(unsafe{ffi::CGEventCreateMouseEvent(self.source.0,kind,point,button.into())});if event.0.is_null(){return Err(io::Error::other("Cannot create native tablet mouse event"));}
        unsafe{ffi::CGEventSetIntegerValueField(event.0,1,if matches!(kind,1|2|3|4|25|26){self.clicks.count}else{0});ffi::CGEventSetIntegerValueField(event.0,3,button.into());ffi::CGEventSetDoubleValueField(event.0,4,delta.x);ffi::CGEventSetDoubleValueField(event.0,5,delta.y);}
        let now=Instant::now();let proximity=buttons==0&&previous==0&&now.duration_since(self.last_proximity)>Duration::from_millis(200);
        if buttons==0&&previous==0{self.last_proximity=now;}
        if proximity{self.proximity()?;unsafe{ffi::CGEventSetIntegerValueField(event.0,7,2);}self.proximity_fields(event.0);}
        else{let pressure=f64::from(self.attributes.pressure.unwrap_or(1.0));let tablet_buttons=if buttons&1!=0{1}else if buttons&2!=0{2}else if buttons&4!=0{4}else{0};unsafe{
            ffi::CGEventSetDoubleValueField(event.0,2,pressure);ffi::CGEventSetIntegerValueField(event.0,7,1);ffi::CGEventSetIntegerValueField(event.0,18,tablet_buttons);ffi::CGEventSetIntegerValueField(event.0,24,DEVICE_ID);ffi::CGEventSetDoubleValueField(event.0,19,pressure);
            if let Some(tilt)=self.attributes.tilt{ffi::CGEventSetDoubleValueField(event.0,20,f64::from(tilt[0])/90.0);ffi::CGEventSetDoubleValueField(event.0,21,-f64::from(tilt[1])/90.0);}
        }}
        unsafe{ffi::CGEventSetFlags(event.0,if flags==0{u32::MAX as u64}else{flags});ffi::CGEventSetTimestamp(event.0,event_timestamp()?);ffi::CGEventPost(0,event.0);}Ok(())
    }
}
#[cfg(test)]mod fixtures{
    use super::*;
    #[test]fn click_window_uses_original_eight_pixel_tolerance_and_down_clock(){
        let now=Instant::now();let mut clicks=Clicks{count:0,started:None,down:ffi::Point::default(),moved:false,interval:Duration::from_millis(500),last_button:0};
        clicks.edge(true,ffi::Point{x:10.0,y:10.0},now);clicks.edge(false,ffi::Point{x:18.0,y:10.0},now+Duration::from_millis(100));assert_eq!(clicks.count,1);
        clicks.edge(true,ffi::Point{x:18.0,y:10.0},now+Duration::from_millis(200));assert_eq!(clicks.count,2);
        clicks.movement(ffi::Point{x:18.1,y:10.0});clicks.edge(false,ffi::Point{x:18.1,y:10.0},now+Duration::from_millis(250));assert_eq!(clicks.count,0);
        clicks.edge(true,ffi::Point{x:18.1,y:10.0},now+Duration::from_millis(300));assert_eq!(clicks.count,1);
        clicks.edge(false,ffi::Point{x:18.1,y:10.0},now+Duration::from_millis(801));assert_eq!(clicks.count,0);
    }
}

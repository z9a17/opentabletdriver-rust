//! Service-only I/O over the physical reader's existing owned handle.
use std::{io,ptr,sync::{Mutex,atomic::{AtomicBool,Ordering}}};
use crate::{hid::{OwnedHandle,Event},winusb::Interface,managed_services::Operation};
use windows_sys::Win32::{Foundation::{ERROR_IO_PENDING,WAIT_OBJECT_0,WAIT_TIMEOUT},
    Storage::FileSystem::WriteFile,System::{IO::{CancelIoEx,DeviceIoControl,GetOverlappedResult,OVERLAPPED},
    Threading::WaitForMultipleObjects},Devices::HumanInterfaceDevice::HidD_GetIndexedString};

pub struct SharedIo {
    // Interface must be freed before the backing file handle.
    pub winusb:Option<Interface>,pub handle:Option<OwnedHandle>,custom:Option<std::sync::Arc<crate::custom_devices::Handle>>,
    stop:Event,gate:Mutex<()>,closed:AtomicBool,output_length:u32,feature_length:u32,
}
// Windows overlapped handles and WinUSB interfaces support concurrent requests.
// Every operation owns its own event/OVERLAPPED/buffer; service writes/control
// calls are serialized separately from native input. Arc pins the handle and
// interface until admitted requests finish.
unsafe impl Send for SharedIo {}
unsafe impl Sync for SharedIo {}
impl SharedIo {
    pub fn new(handle:OwnedHandle,winusb:Option<Interface>,output_length:u32,feature_length:u32)->io::Result<Self>{
        Ok(Self{handle:Some(handle),winusb,custom:None,stop:Event::create(true)?,gate:Mutex::new(()),closed:AtomicBool::new(false),output_length,feature_length})
    }
    pub fn new_custom(custom:std::sync::Arc<crate::custom_devices::Handle>,output_length:u32,feature_length:u32)->io::Result<Self> {
        Ok(Self{handle:None,winusb:None,custom:Some(custom),stop:Event::create(true)?,gate:Mutex::new(()),closed:AtomicBool::new(false),output_length,feature_length})
    }
    pub fn native_handle(&self)->io::Result<&OwnedHandle>{self.handle.as_ref().ok_or_else(||io::Error::other("Endpoint is owned by an original custom hub"))}
    pub fn cancel_services(&self){self.closed.store(true,Ordering::Release);let _=self.stop.signal();}
    pub fn string(&self,index:u8)->io::Result<String>{
        let _guard=self.gate.lock().map_err(|_|io::Error::other("Device I/O poisoned"))?;self.check()?;
        if let Some(custom)=&self.custom{return custom.string(index);}
        if let Some(interface)=&self.winusb{return interface.service_string(index,&self.stop);}
        let mut text=[0u16;1024];
        if !unsafe{HidD_GetIndexedString(self.native_handle()?.raw(),u32::from(index),text.as_mut_ptr().cast(),2048)}{return Err(io::Error::last_os_error());}
        let length=text.iter().position(|word|*word==0).unwrap_or(text.len());
        String::from_utf16(&text[..length]).map_err(|e|io::Error::new(io::ErrorKind::InvalidData,e))
    }
    fn check(&self)->io::Result<()>{if self.closed.load(Ordering::Acquire){Err(io::Error::new(io::ErrorKind::BrokenPipe,"Physical reader retired"))}else{Ok(())}}
    pub fn service(&self,operation:Operation,data:&mut Vec<u8>)->io::Result<()> {
        let _guard=self.gate.lock().map_err(|_|io::Error::other("Device I/O poisoned"))?;self.check()?;
        let length=(if operation==Operation::WriteStream{self.output_length}else{self.feature_length})as usize;
        if length==0||length>65535||data.is_empty()||data.len()>length{return Err(io::Error::new(io::ErrorKind::InvalidInput,"Report exceeds endpoint capability"));}
        let requested=data.len();data.resize(length,0);
        if let Some(custom)=&self.custom {
            match operation {Operation::WriteStream=>custom.write(data)?,Operation::GetFeature=>custom.get_feature(data)?,Operation::SetFeature=>custom.set_feature(data)?,_=>return Err(io::Error::other("Invalid custom endpoint operation"))}
        }
        else if let Some(interface)=&self.winusb{interface.service(operation,data,&self.stop)?;}
        else {
            let event=Event::create(true)?;let mut overlapped=OVERLAPPED{hEvent:event.raw(),..Default::default()};
            let mut count=0;
            // IOCTL definitions and in/out semantics from Windows hidclass.h
            // and the installed hidapi Windows backend.
            let ioctl=match operation{Operation::GetFeature=>0x000b0192,Operation::SetFeature=>0x000b0191,_=>0};
            let started=unsafe{if operation==Operation::WriteStream{
                WriteFile(self.native_handle()?.raw(),data.as_ptr(),data.len()as u32,ptr::null_mut(),&mut overlapped)
            }else{DeviceIoControl(self.native_handle()?.raw(),ioctl,data.as_mut_ptr().cast(),data.len()as u32,
                data.as_mut_ptr().cast(),data.len()as u32,&mut count,&mut overlapped)}};
            if started==0 {
                let error=io::Error::last_os_error();if error.raw_os_error()!=Some(ERROR_IO_PENDING as i32){return Err(error);}
                let waited=unsafe{WaitForMultipleObjects(2,[self.stop.raw(),event.raw()].as_ptr(),0,5000)};
                if waited!=WAIT_OBJECT_0+1 {
                    unsafe{CancelIoEx(self.native_handle()?.raw(),&overlapped);GetOverlappedResult(self.native_handle()?.raw(),&overlapped,&mut count,1);}
                    return Err(if waited==WAIT_TIMEOUT{io::Error::new(io::ErrorKind::TimedOut,"Device I/O timed out")}
                        else{io::Error::new(io::ErrorKind::Interrupted,"Device I/O cancelled")});
                }
            }
            if unsafe{GetOverlappedResult(self.native_handle()?.raw(),&overlapped,&mut count,0)}==0{return Err(io::Error::last_os_error());}
            if operation==Operation::WriteStream && count as usize!=data.len(){return Err(io::Error::new(io::ErrorKind::WriteZero,"Partial device output write"));}
            if operation==Operation::GetFeature {
                let length=(count as usize).saturating_add(usize::from(data[0]==0));
                if length>data.len()||length<requested{return Err(io::Error::new(io::ErrorKind::UnexpectedEof,"Feature report returned an invalid length"));}
            }
        }
        if operation==Operation::GetFeature{data.truncate(requested);}self.check()
    }
}

//! Actual custom hub endpoint exports. Metadata/operations remain off the
//! native-only report path; original streams belong to a dedicated reader.
use super::{bridge, last_error};
use std::ffi::c_void;
use serde::Deserialize;

type Devices = unsafe extern "C" fn(*mut u8, i32, i32) -> i32;
type Open = unsafe extern "C" fn(u64) -> u64;
type Read = unsafe extern "C" fn(u64, *mut u8, i32) -> i32;
type Buffer = unsafe extern "C" fn(u64, *mut u8, u32) -> i32;
type Close = unsafe extern "C" fn(u64) -> i32;
type StringQuery = unsafe extern "C" fn(u64, u8, *mut u8, i32) -> i32;
pub(super) struct Api { devices: Devices, open: Open, read: Read,
    write: Buffer, get: Buffer, set: Buffer, close: Close, string: StringQuery }
impl Api {
    pub(super) fn load(entry: &impl Fn(&str) -> Result<*mut c_void,String>) -> Result<Self,String> {
        Ok(unsafe { Self {
            devices:std::mem::transmute::<*mut c_void,Devices>(entry("GetHostedDevices")?),
            open:std::mem::transmute::<*mut c_void,Open>(entry("OpenHostedDevice")?),
            read:std::mem::transmute::<*mut c_void,Read>(entry("ReadHostedDevice")?),
            write:std::mem::transmute::<*mut c_void,Buffer>(entry("WriteHostedDevice")?),
            get:std::mem::transmute::<*mut c_void,Buffer>(entry("GetHostedFeature")?),
            set:std::mem::transmute::<*mut c_void,Buffer>(entry("SetHostedFeature")?),
            close:std::mem::transmute::<*mut c_void,Close>(entry("CloseHostedDevice")?),
            string:std::mem::transmute::<*mut c_void,StringQuery>(entry("GetHostedDeviceString")?),
        } })
    }
}
fn api() -> Result<&'static Api,String> { bridge()?.custom_devices.as_ref()
    .ok_or_else(||"Installed managed bridge lacks actual custom stream exports".into()) }
#[derive(Clone,Debug,Deserialize)]
pub struct Metadata {
    pub endpoint:u64, pub scope:u64,
    #[serde(rename="DevicePath")] pub path:String,
    #[serde(rename="VendorID")] pub vendor:u16,
    #[serde(rename="ProductID")] pub product:u16,
    #[serde(rename="InputReportLength")] pub input_length:u16,
    #[serde(rename="OutputReportLength")] pub output_length:u16,
    #[serde(rename="FeatureReportLength")] pub feature_length:u16,
    #[serde(rename="CanOpen")] pub can_open:bool,
    #[serde(rename="DeviceAttributes")] pub attributes:Option<std::collections::BTreeMap<String,String>>,
    #[serde(flatten)] pub original:std::collections::BTreeMap<String,serde_json::Value>,
}
pub fn snapshot() -> Result<Vec<Metadata>,String> {
    // Discovery is passive for profiles that have never loaded managed code.
    if !super::initialized() { return Ok(Vec::new()); }
    let api=api()?;
    let size=unsafe{(api.devices)(std::ptr::null_mut(),0,1)};
    if size<0 {return Err(last_error());}
    if !(1..=4194304).contains(&size) {return Err("Invalid custom endpoint metadata size".into());}
    let mut bytes=vec![0;size as usize];
    if unsafe{(api.devices)(bytes.as_mut_ptr(),size,0)}!=size {return Err(last_error());}
    let entries:Vec<Metadata>=serde_json::from_slice(&bytes).map_err(|e|e.to_string())?;
    if entries.len()>1024 || entries.iter().any(|e|e.endpoint==0 || e.path.is_empty() || e.path.len()>32768 || e.path.contains('\0') || e.input_length==0) {
        return Err("Invalid actual custom endpoint metadata".into());
    }
    Ok(entries)
}
impl Metadata {
    pub fn original_json(&self)->serde_json::Value {
        let mut values:serde_json::Map<String,serde_json::Value>=self.original.clone().into_iter().collect();
        values.extend(serde_json::json!({"endpoint":self.endpoint,"scope":self.scope,"DevicePath":self.path,
            "VendorID":self.vendor,"ProductID":self.product,"InputReportLength":self.input_length,
            "OutputReportLength":self.output_length,"FeatureReportLength":self.feature_length,
            "CanOpen":self.can_open,"DeviceAttributes":self.attributes}).as_object().expect("metadata object").clone());
        serde_json::Value::Object(values)
    }
}
pub fn device_string(endpoint:u64,index:u8)->Result<String,String> {
    let api=api()?;let size=unsafe{(api.string)(endpoint,index,std::ptr::null_mut(),0)};
    if size<0{return Err(last_error());}if size>4194304{return Err("Custom string exceeds 4 MiB".into());}
    if size==0{return Ok(String::new());}
    let mut bytes=vec![0;size as usize];
    if unsafe{(api.string)(endpoint,index,bytes.as_mut_ptr(),size)}!=size{return Err(last_error());}
    String::from_utf8(bytes).map_err(|e|e.to_string())
}
pub fn open(endpoint:u64)->Result<u64,String> {let token=unsafe{(api()?.open)(endpoint)};if token==0{Err(last_error())}else{Ok(token)}}
pub fn read(token:u64,buffer:&mut [u8])->Result<usize,String> {
    if buffer.is_empty() || buffer.len()>65535{return Err("Invalid custom read buffer".into());}
    let length=unsafe{(api()?.read)(token,buffer.as_mut_ptr(),buffer.len() as i32)};
    if length<0{return Err(last_error());}
    if length==0 || length as usize>buffer.len(){return Err("Original stream report exceeds its declared input length".into());}
    Ok(length as usize)
}
pub fn write(token:u64,data:&[u8])->Result<(),String> {
    if data.is_empty()||data.len()>65535{return Err("Invalid custom write buffer".into());}
    if unsafe{(api()?.write)(token,data.as_ptr() as *mut u8,data.len() as u32)}<0{Err(last_error())}else{Ok(())}
}
pub fn get_feature(token:u64,data:&mut[u8])->Result<(),String> {
    if data.is_empty()||data.len()>65535{return Err("Invalid custom feature buffer".into());}
    if unsafe{(api()?.get)(token,data.as_mut_ptr(),data.len() as u32)}<0{Err(last_error())}else{Ok(())}
}
pub fn set_feature(token:u64,data:&[u8])->Result<(),String> {
    if data.is_empty()||data.len()>65535{return Err("Invalid custom feature buffer".into());}
    if unsafe{(api()?.set)(token,data.as_ptr() as *mut u8,data.len() as u32)}<0{Err(last_error())}else{Ok(())}
}
pub fn close(token:u64)->Result<(),String>{if unsafe{(api()?.close)(token)}<0{Err(last_error())}else{Ok(())}}

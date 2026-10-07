//! A real original custom stream feeds the same native endpoint broker. Only
//! the dedicated managed reader calls Read; this source polls its fixed ring.
use std::{io,sync::{Arc,atomic::{AtomicBool,Ordering}},time::{Duration,Instant}};
use otd_core::{endpoint_match::{Endpoint,Transport},session::{Read,ReportSource},tablets::{Database,TabletConfiguration,DeviceIdentifier}};
use crate::{custom_devices,shared_device_io::{OutputWake,ReaderServices,SharedIo,RequestKind},shared_devices::{Registration,OutputGate},managed_services::Operation};
pub fn endpoints(database:&Database)->Result<Vec<(Endpoint,u64)>,String>{
    let mut result=Vec::new();
    for metadata in crate::dotnet::custom_devices::snapshot()?{
        if database.find(metadata.vendor,metadata.product).next().is_none(){continue;}
        let physical=metadata.attributes.as_ref().and_then(|attributes|attributes.get("OTD_PHYSICAL_ID")).cloned().unwrap_or_else(||format!("managed-hub:{}",metadata.scope));
        // Match the shipped Windows custom-owner contract: ordinary HID report
        // identifier semantics, while the actual backend remains the hub token.
        let mut endpoint=Endpoint{path:metadata.path,physical_id:physical,transport:Transport::UsbHid,vendor_id:metadata.vendor,product_id:metadata.product,can_open:metadata.can_open,
            input_length:metadata.input_length.into(),output_length:metadata.output_length.into(),feature_length:metadata.feature_length.into(),strings:Default::default(),attributes:metadata.attributes};
        let indices=database.find(endpoint.vendor_id,endpoint.product_id).filter(|candidate|otd_core::endpoint_match::matches_report_lengths(&endpoint,candidate.identifier))
            .flat_map(|candidate|candidate.identifier.device_strings.iter().flat_map(|strings|strings.keys())).filter_map(|index|index.parse::<u8>().ok()).collect::<std::collections::BTreeSet<_>>();
        for index in indices{match crate::dotnet::custom_devices::device_string(metadata.endpoint,index){Ok(value)=>{endpoint.strings.insert(index,value);},Err(error)=>eprintln!("Custom endpoint matching string {index}: {error}")}}
        result.push((endpoint,metadata.endpoint));
    }Ok(result)
}
pub fn inventory()->Result<Vec<serde_json::Value>,String>{crate::dotnet::custom_devices::snapshot().map(|metadata|metadata.into_iter().map(|entry|{
    let mut value=serde_json::json!(entry.original);let object=value.as_object_mut().expect("metadata object");
    object.extend(serde_json::json!({"DevicePath":entry.path,"VendorID":entry.vendor,"ProductID":entry.product,"CanOpen":entry.can_open,
        "InputReportLength":entry.input_length,"OutputReportLength":entry.output_length,"FeatureReportLength":entry.feature_length,"DeviceAttributes":entry.attributes,"custom_endpoint":entry.endpoint}).as_object().unwrap().clone());value
}).collect())}
pub struct Source<'a>{reader:custom_devices::Reader,wake:Arc<OutputWake>,services:ReaderServices,registration:Registration,buffer:Box<[u8]>,stop:&'a AtomicBool,label:String}
impl<'a> Source<'a>{
    pub fn open(endpoint:u64,device:&Endpoint,configuration:&TabletConfiguration,identifier:&DeviceIdentifier,auxiliary:bool,gate:Arc<OutputGate>,epoch:Option<u64>,stop:&'a AtomicBool)->io::Result<Self>{
        let wake=Arc::new(OutputWake::create(true)?);let signal=wake.clone();let reader=custom_devices::Reader::open(endpoint,device.input_length as usize,Arc::new(move||{let _=signal.signal();}))?;
        let(io,services)=SharedIo::new(device.output_length,device.feature_length)?;
        let registration=Registration::new(device.path.clone(),identifier.parser().into(),auxiliary,io,gate,device.input_length as usize,
            serde_json::json!({"Properties":configuration,"Identifiers":[identifier]}),serde_json::json!(identifier),serde_json::json!(configuration),epoch).map_err(io::Error::other)?;
        Ok(Self{reader,wake,services,registration,buffer:vec![0;device.input_length as usize].into_boxed_slice(),stop,label:format!("{} ({})",configuration.name,device.path)})
    }
    pub fn tablet(&self,tablet:serde_json::Value){self.registration.tablet(tablet);}
    pub fn initialized(&self){self.registration.endpoint.initialized.store(true,Ordering::Release);}
    pub fn initialize(&self,identifier:&DeviceIdentifier,configuration:&TabletConfiguration,device:&Endpoint)->io::Result<()>{
        let handle=self.reader.handle();for index in identifier.initialization_strings.iter().flatten(){if self.stop.load(Ordering::Acquire){return Err(io::Error::new(io::ErrorKind::Interrupted,"Initialization cancelled"));}let _=handle.string(*index)?;}
        let delay=configuration.attributes.as_ref().and_then(|attributes|attributes.get("FeatureInitDelayMs")).map(|value|value.parse::<u32>()).transpose().map_err(io::Error::other)?.unwrap_or(0);
        if delay==u32::MAX{return Err(io::Error::new(io::ErrorKind::InvalidInput,"Infinite initialization delay"));}
        for(feature,length,reports)in[(true,device.feature_length,identifier.feature_init_report.as_deref()),(false,device.output_length,identifier.output_init_report.as_deref())]{
            for report in reports.unwrap_or_default().iter().filter(|report|!report.0.is_empty()){
                if length==0||length>65535||report.0.len()>length as usize{return Err(io::Error::new(io::ErrorKind::InvalidInput,"Initialization exceeds actual custom endpoint report capability"));}
                if feature{let until=Instant::now()+Duration::from_millis(delay.into());while Instant::now()<until{if self.stop.load(Ordering::Acquire){return Err(io::Error::new(io::ErrorKind::Interrupted,"Initialization cancelled"));}std::thread::sleep(until.saturating_duration_since(Instant::now()).min(Duration::from_millis(50)));}}
                let mut bytes=vec![0;length as usize];bytes[..report.0.len()].copy_from_slice(&report.0);if feature{handle.set_feature(&bytes)?;}else{handle.write(&bytes)?;}
            }
        }Ok(())
    }
    pub fn waits(&self)->[libc::pollfd;3]{[libc::pollfd{fd:self.wake.raw(),events:libc::POLLIN,revents:0},libc::pollfd{fd:self.services.wake().raw(),events:libc::POLLIN,revents:0},libc::pollfd{fd:self.registration.endpoint.output.reader_wake().raw(),events:libc::POLLIN,revents:0}]}
    pub fn close(&mut self)->io::Result<()>{self.registration.endpoint.io.cancel_services();self.reader.close(Duration::from_secs(3))}
}
impl ReportSource for Source<'_>{
    fn label(&self)->&str{&self.label}fn now(&self)->Instant{Instant::now()}
    fn native_output_enabled(&self)->bool{self.registration.endpoint.output.native_enabled()}
    fn output_started(&self){self.registration.endpoint.output.start()}
    fn output_acknowledged(&self,enabled:bool){self.registration.endpoint.output.acknowledge(enabled)}
    fn next(&mut self,timeout:Duration)->io::Result<Read<'_>>{
        if self.stop.load(Ordering::Acquire){return Ok(Read::Ended);}
        if self.registration.endpoint.output.transition_pending(){return Ok(Read::Idle);}
        let handle=self.reader.handle();self.services.drain(|kind,data|match kind{RequestKind::String(index)=>handle.string(index).map(|value|*data=value.into_bytes()),RequestKind::Report(Operation::WriteStream)=>handle.write(data),RequestKind::Report(Operation::GetFeature)=>handle.get_feature(data),RequestKind::Report(Operation::SetFeature)=>handle.set_feature(data),_=>Err(io::Error::new(io::ErrorKind::InvalidInput,"Unknown original stream service"))});
        if let Some((length,ready))=self.reader.poll_report(&mut self.buffer)?{self.registration.publish(&self.buffer[..length]);return Ok(Read::Report{bytes:&self.buffer[..length],ready,queued:true});}
        let mut waits=self.waits();let result=unsafe{libc::poll(waits.as_mut_ptr(),waits.len() as libc::nfds_t,timeout.min(Duration::from_secs(1)).as_micros().div_ceil(1000) as i32)};
        if result<0{let error=io::Error::last_os_error();if error.kind()!=io::ErrorKind::Interrupted{return Err(error);}}
        self.wake.drain();self.registration.endpoint.output.reader_wake().drain();Ok(Read::Idle)
    }
}

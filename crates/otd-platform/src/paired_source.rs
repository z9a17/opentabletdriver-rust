//! Two independently owned endpoint parsers share one physical output owner.
//! The platform waits on both endpoints without moving either native handle.
use std::{io,time::{Duration,Instant}};
use otd_core::session::{Read,ReportSource};
enum ResultKind{Report(usize,Instant,bool),Idle,Ended}
pub struct PairedSource<S,W>{primary:S,auxiliary:Option<S>,wait:W,delivery:Box<[u8]>,aux_first:bool}
impl<S,W> PairedSource<S,W>{pub fn new(primary:S,auxiliary:Option<S>,wait:W)->Self{Self{primary,auxiliary,wait,delivery:vec![0;65535].into_boxed_slice(),aux_first:false}}}
fn copy(source:&mut impl ReportSource,delivery:&mut[u8])->io::Result<ResultKind>{
    match source.next(Duration::ZERO)?{
        Read::Report{bytes,ready,queued}|Read::Auxiliary{bytes,ready,queued}=>{if bytes.len()>delivery.len(){return Err(io::Error::new(io::ErrorKind::InvalidData,"Endpoint exceeds paired report bound"));}delivery[..bytes.len()].copy_from_slice(bytes);Ok(ResultKind::Report(bytes.len(),ready,queued))},
        Read::Ended|Read::AuxiliaryEnded=>Ok(ResultKind::Ended),Read::Idle=>Ok(ResultKind::Idle),
    }
}
impl<S:ReportSource,W:FnMut(&mut S,Option<&mut S>,Duration)->io::Result<()>> ReportSource for PairedSource<S,W>{
    fn label(&self)->&str{self.primary.label()}
    fn now(&self)->Instant{self.primary.now()}
    fn shared_output(&self)->bool{self.primary.shared_output()}
    fn native_output_enabled(&self)->bool{self.primary.native_output_enabled()}
    fn output_started(&self){self.primary.output_started()}
    fn output_acknowledged(&self,enabled:bool){self.primary.output_acknowledged(enabled)}
    fn next(&mut self,timeout:Duration)->io::Result<Read<'_>>{
        let deadline=Instant::now()+timeout;
        loop{
            // Rotate priority when both streams are continuously readable.
            for auxiliary in [self.aux_first,!self.aux_first]{
                let result=if auxiliary{match &mut self.auxiliary{Some(source)=>copy(source,&mut self.delivery),None=>continue}}else{copy(&mut self.primary,&mut self.delivery)};
                match result{
                    Ok(ResultKind::Report(length,ready,queued))=>{self.aux_first=!auxiliary;return Ok(if auxiliary{Read::Auxiliary{bytes:&self.delivery[..length],ready,queued}}else{Read::Report{bytes:&self.delivery[..length],ready,queued}});},
                    Err(error)if auxiliary=>{eprintln!("Auxiliary endpoint retired: {error}");self.auxiliary=None;return Ok(Read::AuxiliaryEnded);},
                    Ok(ResultKind::Ended)if auxiliary=>{self.auxiliary=None;return Ok(Read::AuxiliaryEnded);},
                    Ok(ResultKind::Ended)=>return Ok(Read::Ended),Err(error)=>return Err(error),Ok(ResultKind::Idle)=>{},
                }
            }
            let remaining=deadline.saturating_duration_since(Instant::now());if remaining.is_zero(){return Ok(Read::Idle);}
            (self.wait)(&mut self.primary,self.auxiliary.as_mut(),remaining)?;
            // Output authority changes wake even when neither endpoint emits
            // a report. Core performs its release/reset before acknowledging.
            if !self.primary.native_output_enabled(){return Ok(Read::Idle);}
        }
    }
}

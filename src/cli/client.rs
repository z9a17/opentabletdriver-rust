//! One persistent daemon-owned Console connection per invocation/read-modify-write.
use std::sync::{Arc,atomic::{AtomicBool,Ordering}};
use std::time::{Duration, Instant};
use serde_json::{Value, json};
use crate::control::pipe::CompatPipe;
use crate::upstream_rpc::protocol;

pub struct Client { pipe:CompatPipe, next:u64, stop:Arc<AtomicBool>, failed:bool }
impl Client {
    pub fn connect() -> Result<Self,String> {Self::connect_cancellable(Arc::new(AtomicBool::new(false)))}
    pub fn connect_cancellable(stop:Arc<AtomicBool>)->Result<Self,String>{
        if stop.load(Ordering::Acquire){return Err("console request cancelled".into());}
        crate::daemon::call(crate::control::Command::Status)?;
        let endpoint=crate::control::endpoint_name().map_err(|error|error.to_string())?;
        let pid=CompatPipe::server_process_id(&endpoint).map_err(|error|error.to_string())?;
        let pipe = CompatPipe::open_client(crate::upstream_rpc::CONSOLE_PIPE,
            Instant::now()+Duration::from_secs(5),Some(pid)).map_err(|error| error.to_string())?;
        Ok(Self {pipe,next:1,stop,failed:false})
    }
    pub fn usable(&self)->bool{!self.failed}
    pub fn call(&mut self,method:&str,params:Value) -> Result<Value,String> {self.call_with_timeout(method,params,Duration::from_secs(120))}
    pub fn call_with_timeout(&mut self,method:&str,params:Value,timeout:Duration)->Result<Value,String>{
        if self.failed { return Err("console connection failed; mutation outcome may be unknown; reconnect and inspect settings before retrying".into()); }
        let id=self.next;
        self.next=self.next.checked_add(1).ok_or("console request sequence exhausted")?;
        let request=protocol::encode(&json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}))
            .map_err(|error|error.to_string())?;
        let deadline=Instant::now()+timeout;
        let result:Result<Result<Value,String>,String>=(|| {
            self.pipe.write(&request,deadline,&self.stop).map_err(|error|error.to_string())?;
            loop {
                let mut header=Vec::new();
                while !header.ends_with(b"\r\n\r\n") {
                    if header.len()>=protocol::MAX_HEADER {return Err("console response header exceeds bound".into());}
                    let mut byte=[0];self.read_exact(&mut byte,deadline)?;header.push(byte[0]);
                }
                let length=protocol::body_length(&header).map_err(|error|error.to_string())?;
                let mut body=vec![0;length];self.read_exact(&mut body,deadline)?;
                let reply:Value=serde_json::from_slice(&body).map_err(|error|format!("invalid console response: {error}"))?;
                if reply["jsonrpc"]!="2.0" {return Err("invalid console protocol version".into());}
                // Original service emits bounded notifications between replies.
                if reply.get("id").is_none() && reply["method"].is_string() {continue;}
                if reply["id"]!=id {return Err("console response correlation mismatch".into());}
                if let Some(error)=reply.get("error") {
                    return Ok(Err(format!("{}: {}",error["code"],error["message"].as_str().unwrap_or("daemon operation failed"))));
                }
                return reply.get("result").cloned().map(Ok).ok_or_else(||"console response has no result".into());
            }
        })();
        match result { Ok(value)=>value,Err(error)=>{self.failed=true;Err(format!("{method}: {error}; request may already have taken effect"))} }
    }
    fn read_exact(&self,mut bytes:&mut[u8],deadline:Instant)->Result<(),String>{
        while !bytes.is_empty(){
            let read=self.pipe.read(bytes,deadline,&self.stop,&mut||{}).map_err(|error|error.to_string())?;
            if read==0{return Err("console daemon disconnected".into());}
            bytes=&mut bytes[read..];
        }Ok(())
    }
}

//! Current-user-only Unix socket. The original .NET RPC is a distinct optional
//! listener; this protocol controls the native owner without initializing CLR.
use std::fs::{File,OpenOptions};
use std::io::{self,Read,Write};
use std::os::fd::AsRawFd;
use std::os::unix::{fs::{DirBuilderExt,FileTypeExt,MetadataExt,OpenOptionsExt,PermissionsExt},net::{UnixListener,UnixStream}};
use std::path::{Path,PathBuf};
use std::sync::{Arc,atomic::{AtomicBool,Ordering}};
use std::thread::JoinHandle;
use std::time::{Duration,Instant};
use serde_json::json;
use crate::daemon::{Command,Handle};

const MAX_FRAME:usize=256*1024;
pub fn endpoint() -> Result<PathBuf,String> { Ok(otd_core::storage::data_directory()?.join("control").join("daemon.sock")) }
fn private_directory(path:&Path) -> io::Result<()> {
    std::fs::DirBuilder::new().recursive(true).mode(0o700).create(path)?;
    let metadata=std::fs::symlink_metadata(path)?;
    if !metadata.is_dir()||metadata.uid()!=unsafe{libc::geteuid()} {return Err(io::Error::new(io::ErrorKind::PermissionDenied,"Control directory must be a real directory owned by the current user"));}
    std::fs::set_permissions(path,std::fs::Permissions::from_mode(0o700))
}
fn peer_is_current(stream:&UnixStream) -> io::Result<bool> {
    #[cfg(target_os="linux")]
    {
        let mut credentials:libc::ucred=unsafe{std::mem::zeroed()};let mut length=std::mem::size_of::<libc::ucred>() as libc::socklen_t;
        if unsafe{libc::getsockopt(stream.as_raw_fd(),libc::SOL_SOCKET,libc::SO_PEERCRED,(&mut credentials as *mut libc::ucred).cast(),&mut length)}!=0{return Err(io::Error::last_os_error());}
        Ok(credentials.uid==unsafe{libc::geteuid()})
    }
    #[cfg(target_os="macos")]
    {
        let(mut uid,mut gid)=(0,0);
        if unsafe{libc::getpeereid(stream.as_raw_fd(),&mut uid,&mut gid)}!=0{return Err(io::Error::last_os_error());}
        Ok(uid==unsafe{libc::geteuid()})
    }
}
fn read_frame(stream:&mut UnixStream,stop:&AtomicBool) -> io::Result<Vec<u8>> {
    let deadline=Instant::now()+Duration::from_secs(10);let mut bytes=Vec::new();let mut byte=[0u8;1];
    loop {
        if stop.load(Ordering::Acquire){return Err(io::Error::new(io::ErrorKind::Interrupted,"Native owner stopping"));}
        if Instant::now()>=deadline{return Err(io::Error::new(io::ErrorKind::TimedOut,"Native control request deadline"));}
        match stream.read(&mut byte){Ok(0)=>return Err(io::Error::new(io::ErrorKind::UnexpectedEof,"Control disconnected")),Ok(_)=>{
            if byte[0]==b'\n'{return Ok(bytes);}if bytes.len()==MAX_FRAME{return Err(io::Error::new(io::ErrorKind::InvalidData,"Native control request exceeds 256 KiB"));}bytes.push(byte[0]);
        },Err(error)if matches!(error.kind(),io::ErrorKind::WouldBlock|io::ErrorKind::TimedOut|io::ErrorKind::Interrupted)=>{},Err(error)=>return Err(error)}
    }
}
pub struct Server {path:PathBuf,device:u64,inode:u64,lock:File,stop:Arc<AtomicBool>,join:Option<JoinHandle<()>>}
impl Server {
    pub fn start(handle:Handle,stop:Arc<AtomicBool>) -> Result<Self,String> {
        let path=endpoint()?;let parent=path.parent().ok_or("Control socket has no directory")?;
        private_directory(parent).map_err(|error|error.to_string())?;
        // Keep the socket and ownership lock under the kernel AF_UNIX limit.
        use std::os::unix::ffi::OsStrExt;
        if path.as_os_str().as_bytes().len()>100{return Err("Native control socket path exceeds AF_UNIX limit; choose a shorter OTD_RUST_PORTABLE_DIR".into());}
        let lock=OpenOptions::new().create(true).read(true).write(true).mode(0o600).custom_flags(libc::O_NOFOLLOW|libc::O_CLOEXEC).open(parent.join("daemon.lock")).map_err(|error|error.to_string())?;
        let metadata=lock.metadata().map_err(|error|error.to_string())?;
        if !metadata.is_file()||metadata.uid()!=unsafe{libc::geteuid()}||metadata.mode()&0o077!=0{return Err("Native ownership lock is not private/current-user owned".into());}
        if unsafe{libc::flock(lock.as_raw_fd(),libc::LOCK_EX|libc::LOCK_NB)}!=0{return Err("A native daemon already owns this control endpoint".into());}
        match std::fs::symlink_metadata(&path){
            Ok(metadata)if metadata.file_type().is_socket()&&metadata.uid()==unsafe{libc::geteuid()}=>std::fs::remove_file(&path).map_err(|error|error.to_string())?,
            Ok(_)=>return Err("Existing native control path is not a current-user socket".into()),
            Err(error)if error.kind()==io::ErrorKind::NotFound=>{},Err(error)=>return Err(error.to_string()),
        }
        let listener=UnixListener::bind(&path).map_err(|error|error.to_string())?;
        std::fs::set_permissions(&path,std::fs::Permissions::from_mode(0o600)).map_err(|error|error.to_string())?;
        let metadata=std::fs::symlink_metadata(&path).map_err(|error|error.to_string())?;
        listener.set_nonblocking(true).map_err(|error|error.to_string())?;
        let cancelled=stop.clone();
        let join=std::thread::Builder::new().name("unix-native-control".into()).spawn(move||{
            let mut clients:Vec<JoinHandle<()>>=Vec::new();
            while !cancelled.load(Ordering::Acquire){
                let mut index=0;while index<clients.len(){if clients[index].is_finished(){let _=clients.swap_remove(index).join();}else{index+=1;}}
                match listener.accept(){Ok((mut stream,_))=>{
                    if clients.len()>=8||!peer_is_current(&stream).unwrap_or(false){continue;}
                    if stream.set_read_timeout(Some(Duration::from_millis(250))).is_err()||stream.set_write_timeout(Some(Duration::from_secs(5))).is_err(){continue;}
                    let handle=handle.clone();let cancelled=cancelled.clone();
                    if let Ok(client)=std::thread::Builder::new().name("unix-control-client".into()).spawn(move||{
                        let result=read_frame(&mut stream,&cancelled).map_err(|error|error.to_string()).and_then(|bytes|{
                            let command:Command=serde_json::from_slice(&bytes).map_err(|error|error.to_string())?;handle.call(command)
                        });
                        let value=match result{Ok(value)=>json!({"ok":true,"result":value}),Err(error)=>json!({"ok":false,"error":error})};
                        let mut encoded=serde_json::to_vec(&value).unwrap_or_default();
                        if encoded.len()>MAX_FRAME{encoded=br#"{"ok":false,"error":"Native control response exceeds 256 KiB"}"#.to_vec();}
                        encoded.push(b'\n');let _=stream.write_all(&encoded);
                    }){clients.push(client);}
                },Err(error)if error.kind()==io::ErrorKind::WouldBlock=>std::thread::sleep(Duration::from_millis(50)),Err(_)=>break}
            }
            for client in clients{let _=client.join();}
        }).map_err(|error|error.to_string())?;
        Ok(Self{path,device:metadata.dev(),inode:metadata.ino(),lock,stop,join:Some(join)})
    }
}
impl Drop for Server {fn drop(&mut self){self.stop.store(true,Ordering::Release);if let Some(join)=self.join.take(){let _=join.join();}
    if std::fs::symlink_metadata(&self.path).is_ok_and(|metadata|metadata.dev()==self.device&&metadata.ino()==self.inode){let _=std::fs::remove_file(&self.path);}
    let _=unsafe{libc::flock(self.lock.as_raw_fd(),libc::LOCK_UN)};
}}
pub fn request(command:Command) -> Result<serde_json::Value,String> {
    let mut stream=UnixStream::connect(endpoint()?).map_err(|error|error.to_string())?;
    if !peer_is_current(&stream).map_err(|error|error.to_string())?{return Err("Native control server is not owned by current user".into());}
    stream.set_read_timeout(Some(Duration::from_secs(60))).map_err(|error|error.to_string())?;
    stream.set_write_timeout(Some(Duration::from_secs(5))).map_err(|error|error.to_string())?;
    let mut bytes=serde_json::to_vec(&command).map_err(|error|error.to_string())?;
    if bytes.len()>MAX_FRAME{return Err("Native control request exceeds 256 KiB".into());}bytes.push(b'\n');stream.write_all(&bytes).map_err(|error|error.to_string())?;
    // Server operations can include cleanup; the response has a separate 60s
    // deadline rather than the 10s admission deadline.
    let mut result=Vec::new();let mut byte=[0u8;1];
    loop{stream.read_exact(&mut byte).map_err(|error|error.to_string())?;if byte[0]==b'\n'{break;}if result.len()==MAX_FRAME{return Err("Native control response exceeds 256 KiB".into());}result.push(byte[0]);}
    let reply:serde_json::Value=serde_json::from_slice(&result).map_err(|error|error.to_string())?;
    if reply["ok"]==true{Ok(reply["result"].clone())}else{Err(reply["error"].as_str().unwrap_or("Malformed control response").into())}
}

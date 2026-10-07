/// Windows calls remain unchanged in the shared cold helpers. Unix child
/// processes have no Windows console-window creation flag to configure.
pub trait CommandExt {fn creation_flags(&mut self,flags:u32)->&mut Self;}
impl CommandExt for std::process::Command {fn creation_flags(&mut self,_flags:u32)->&mut Self{self}}

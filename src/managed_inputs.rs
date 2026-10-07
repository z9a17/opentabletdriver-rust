//! Original platform input scopes join the native action owner. All calls run
//! on the independent input service lane, including cleanup and lease expiry.
use std::{collections::{HashMap,HashSet},io,mem::size_of,sync::{Mutex,OnceLock},time::{Duration,Instant}};
use serde_json::{Value,json};
use otd_core::{actions::{Action,ActionTransition,MouseButton},output::buttons::ActionSink};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{MapVirtualKeyW,MAPVK_VK_TO_VSC_EX,INPUT,INPUT_0,INPUT_KEYBOARD,KEYBDINPUT,KEYEVENTF_KEYUP,SendInput};
use crate::managed_services::Operation;

const MAX_SCOPES:usize=256;
struct Scope {actions:crate::action_output::SessionActions,keys:HashMap<u32,Option<Action>>,buttons:HashMap<u32,Action>,expires:Instant}
#[derive(Default)]
struct State {scopes:HashMap<u64,Scope>,raw:HashMap<u32,HashSet<u64>>}
static STATE:OnceLock<Mutex<State>>=OnceLock::new();
fn state()->&'static Mutex<State>{STATE.get_or_init(||Mutex::new(State::default()))}
fn raw_key(code:u32,pressed:bool)->io::Result<()> {
    let input=INPUT{r#type:INPUT_KEYBOARD,Anonymous:INPUT_0{ki:KEYBDINPUT{wVk:code as u16,wScan:0,dwFlags:if pressed{0}else{KEYEVENTF_KEYUP},time:0,dwExtraInfo:0}}};
    unsafe{windows_sys::Win32::Foundation::SetLastError(0)};
    if unsafe{SendInput(1,&input,size_of::<INPUT>() as i32)}!=1{return Err(io::Error::other("Original key transition was not accepted by SendInput"));}Ok(())
}
fn canonical_key(code:u32)->Option<Action>{
    if let Some(usage)=crate::action_output::usage_for_virtual_key(code){return Some(Action::Key(usage));}
    let scan=unsafe{MapVirtualKeyW(code,MAPVK_VK_TO_VSC_EX)};
    if scan==0{return None;}
    crate::action_output::usage_for_scan_code((scan&0xff) as u16,scan&0xff00==0xe000).map(Action::Key)
}
fn button(code:u32)->Result<Option<Action>,String>{Ok(Some(Action::Mouse(match code{
    0=>return Ok(None),1=>MouseButton::Left,2=>MouseButton::Middle,3=>MouseButton::Right,
    4=>MouseButton::Backward,5=>MouseButton::Forward,_=>return Err("Unknown original mouse button".into())
})))}
fn release(state:&mut State,id:u64)->Result<(),String>{
    let Some(mut scope)=state.scopes.remove(&id) else{return Ok(());};
    let mut failure=None;
    // Unacknowledged native releases remain pending in shared ActionState.
    if let Err(error)=scope.actions.release_all(){failure=Some(error.to_string());}
    let raw:Vec<_>=scope.keys.iter().filter_map(|(code,action)|action.is_none().then_some(*code)).collect();
    for code in raw {
        let Some(owners)=state.raw.get_mut(&code) else{scope.keys.remove(&code);continue;};
        if owners.len()==1&&owners.contains(&id){
            if let Err(error)=raw_key(code,false){failure.get_or_insert(error.to_string());continue;}
        }
        owners.remove(&id);if owners.is_empty(){state.raw.remove(&code);}scope.keys.remove(&code);
    }
    if let Some(error)=failure {state.scopes.insert(id,scope);return Err(error);}
    Ok(())
}
pub fn execute(operation:Operation,id:u64,payload:&Value)->Result<Value,String>{
    if id==0{return Err("Original input scope must be nonzero".into());}
    let mut state=state().lock().map_err(|_|"Original input ownership poisoned")?;
    if operation==Operation::InputRelease{release(&mut state,id)?;return Ok(Value::Null);}
    if operation!=Operation::InputHold{return Err("Invalid original input operation".into());}
    let kind=payload["type"].as_str().ok_or("Original input type required")?;
    let lease=payload["lease_ms"].as_u64().unwrap_or(15000).clamp(1000,60000);
    if !state.scopes.contains_key(&id){
        if kind=="renew"{return Err("Original input scope has expired".into());}
        if state.scopes.len()>=MAX_SCOPES{return Err("Original input scope capacity reached".into());}
        let actions=crate::action_output::SessionActions::new().map_err(|error|error.to_string())?;
        state.scopes.insert(id,Scope{actions,keys:HashMap::new(),buttons:HashMap::new(),expires:Instant::now()+Duration::from_millis(lease)});
    }
    if kind=="renew" {state.scopes.get_mut(&id).unwrap().expires=Instant::now()+Duration::from_millis(lease);return Ok(Value::Null);}
    let code=payload["code"].as_u64().and_then(|code|u32::try_from(code).ok()).ok_or("Original input code required")?;
    let held=payload["held"].as_bool().ok_or("Original input held state required")?;
    let mut scope=state.scopes.remove(&id).unwrap();
    let result=(||->Result<(),String>{match kind{
        "key"=>{
            if payload["platform"].as_str()!=Some("windows")||!(1..=255).contains(&code){return Err("Original Windows keyboard code is invalid".into());}
            let action=scope.keys.get(&code).copied().unwrap_or_else(||canonical_key(code));
            if let Some(action)=action{
                if held&&scope.keys.contains_key(&code){crate::action_output::send_transition(ActionTransition{action,pressed:true}).map_err(|error|error.to_string())?;}
                else{scope.actions.hold(code,action,held).map_err(|error|error.to_string())?;scope.actions.flush().map_err(|error|error.to_string())?;}
            }else{
                let owners=state.raw.entry(code).or_default();
                if held {if owners.is_empty()||owners.contains(&id){raw_key(code,true).map_err(|error|error.to_string())?;}owners.insert(id);}
                else if owners.contains(&id){if owners.len()==1{raw_key(code,false).map_err(|error|error.to_string())?;}owners.remove(&id);}
                if owners.is_empty(){state.raw.remove(&code);}
            }
            if held{scope.keys.insert(code,action);}else{scope.keys.remove(&code);}
        },
        "button"=>{if let Some(action)=button(code)?{
            scope.actions.hold(256+code,action,held).map_err(|error|error.to_string())?;scope.actions.flush().map_err(|error|error.to_string())?;
            if held{scope.buttons.insert(code,action);}else{scope.buttons.remove(&code);}
        }},
        _=>return Err("Unknown original input type".into())
    }Ok(())})();
    scope.expires=Instant::now()+Duration::from_millis(lease);state.scopes.insert(id,scope);
    result.map(|_|json!(null))
}
pub fn maintain(){
    if let Ok(mut state)=state().lock(){let now=Instant::now();let expired:Vec<_>=state.scopes.iter().filter_map(|(id,scope)|(now>=scope.expires).then_some(*id)).collect();
        for id in expired{if let Err(error)=release(&mut state,id){if let Some(scope)=state.scopes.get_mut(&id){scope.expires=now+Duration::from_secs(1);}eprintln!("Original input lease cleanup failed: {error}");}}
    }
}
pub fn shutdown(){if let Ok(mut state)=state().lock(){let ids:Vec<_>=state.scopes.keys().copied().collect();for id in ids{if let Err(error)=release(&mut state,id){eprintln!("Original input scope cleanup failed: {error}");}}}}

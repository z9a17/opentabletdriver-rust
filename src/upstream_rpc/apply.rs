//! Per-device guarded apply and owned-generation rollback. This follows the
//! upstream last-valid-settings behavior without claiming group atomicity.
use std::time::Instant;
use crate::device_sessions::SessionReceipt;
use super::protocol::Error;

pub struct Plan { pub id:String, pub generation:u64, pub before:String, pub replacement:String, pub before_collection:bool }
pub trait Backend {
    fn apply(&mut self, id:&str, generation:u64, text:&str) -> Result<SessionReceipt,Error>;
    fn wait(&mut self, receipt:&SessionReceipt, text:&str, deadline:Instant) -> Result<(),Error>;
    fn restore(&mut self, plan:&Plan, receipt:&SessionReceipt, deadline:Instant) -> Result<(),Error>;
}
pub fn run(plans:&[Plan], backend:&mut impl Backend, deadline:Instant,
    rollback_deadline:impl FnOnce() -> Instant) -> Result<(),Error> {
    let mut accepted = Vec::new();
    let result = (|| {
        for plan in plans {
            let receipt = backend.apply(&plan.id,plan.generation,&plan.replacement)?;
            if receipt.id != plan.id || receipt.device_generation != plan.generation
                || receipt.target_generation <= receipt.device_generation {
                return Err(Error::failed("device apply returned an invalid ownership receipt; query settings"));
            }
            // Retain acceptance before waiting: a failed/uncertain completion
            // may still have committed and needs generation-guarded recovery.
            accepted.push((plan,receipt));
            let (plan,receipt) = accepted.last().unwrap();
            backend.wait(receipt,&plan.replacement,deadline)?;
        }
        Ok(())
    })();
    if let Err(failure) = result {
        let deadline = rollback_deadline();
        let mut unresolved = Vec::new();
        for (plan,receipt) in accepted.into_iter().rev() {
            if let Err(error) = backend.restore(plan,&receipt,deadline) {
                unresolved.push(format!("{}: {}",plan.id,error.message));
            }
        }
        return Err(Error::failed(if unresolved.is_empty() {
            format!("{}; accepted device changes reverted or already retained prior settings",failure.message)
        } else {
            format!("{}; partial/uncertain settings apply; rollback did not overwrite newer or pending operations: {}",failure.message,unresolved.join("; "))
        }));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    struct Fake { states:BTreeMap<String,(u64,String)>, peer_change:bool, calls:Vec<String> }
    impl Backend for Fake {
        fn apply(&mut self,id:&str,generation:u64,text:&str) -> Result<SessionReceipt,Error> {
            self.calls.push(format!("{id}:{text}"));
            if id == "second" {
                if self.peer_change { self.states.insert("first".into(),(9,"peer".into())); }
                return Err(Error::failed("second device preparation failed"));
            }
            let state = self.states.get_mut(id).unwrap();
            if state.0 != generation { return Err(Error::failed("generation conflict")); }
            *state = (generation+1,text.into());
            Ok(SessionReceipt{id:id.into(),device_generation:generation,target_generation:generation+1,accepted_pending:false})
        }
        fn wait(&mut self,_:&SessionReceipt,_:&str,_:Instant) -> Result<(),Error> { Ok(()) }
        fn restore(&mut self,plan:&Plan,receipt:&SessionReceipt,_:Instant) -> Result<(),Error> {
            self.apply(&plan.id,receipt.target_generation,&plan.before).map(|_| ())
        }
    }
    #[test]
    fn later_device_failure_restores_owned_generation_but_preserves_a_peer_edit() {
        let plans = [Plan{id:"first".into(),generation:1,before:"old".into(),replacement:"new".into(),before_collection:false},
            Plan{id:"second".into(),generation:4,before:"old2".into(),replacement:"new2".into(),before_collection:false}];
        for peer_change in [false,true] {
            let mut backend = Fake{states:BTreeMap::from([("first".into(),(1,"old".into()))]),peer_change,calls:Vec::new()};
            let failure = run(&plans,&mut backend,Instant::now(),Instant::now).unwrap_err();
            if peer_change {
                assert_eq!(backend.states["first"],(9,"peer".into()));
                assert!(failure.message.contains("partial/uncertain"));
            } else {
                assert_eq!(backend.states["first"],(3,"old".into()));
                assert!(failure.message.contains("reverted"));
            }
        }
    }
}

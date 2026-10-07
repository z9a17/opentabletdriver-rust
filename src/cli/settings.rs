//! Original collection edits retain unknown fields, inactive rows and store order.
use serde_json::{Value,json};
use super::client::Client;

pub fn profile_mut<'a>(document:&'a mut Value,name:&str)->Result<&'a mut Value,String>{
    document["Profiles"].as_array_mut().ok_or("daemon settings has no Profiles")?.iter_mut()
        .find(|row|row["Tablet"].as_str().is_some_and(|tablet|tablet.to_lowercase()==name.to_lowercase()))
        .ok_or_else(||format!("Cannot find profile for tablet '{name}'"))
}
pub fn profile<'a>(document:&'a Value,name:&str)->Result<&'a Value,String>{
    document["Profiles"].as_array().ok_or("daemon settings has no Profiles")?.iter()
        .find(|row|row["Tablet"].as_str().is_some_and(|tablet|tablet.to_lowercase()==name.to_lowercase()))
        .ok_or_else(||format!("Cannot find profile for tablet '{name}'"))
}
pub fn store(client:&mut Client,path:&str,category:&str)->Result<Value,String>{
    client.call("ConstructPluginStore",json!([path,category]))
}
pub fn change_stores(client:&mut Client,stores:&mut Value,paths:&[String],category:&str,operation:&str)->Result<(),String>{
    let rows=stores.as_array_mut().ok_or("plugin collection is not an array")?;
    for path in paths {
        if operation=="disable"{
            let mut found=false;
            for row in rows.iter_mut().filter(|row|row["Path"].as_str()==Some(path)) {row["Enable"]=json!(false);found=true;}
            if !found {println!("No plugins found matching path");}
        }else{
            if operation=="reset" {rows.retain(|row|row["Path"].as_str()!=Some(path));}
            if let Some(row)=rows.iter_mut().find(|row|row["Path"].as_str()==Some(path)) {row["Enable"]=json!(true);}
            else{rows.push(store(client,path,category)?);}
        }
    }Ok(())
}
pub fn format_store(store:&Value)->Option<String>{
    if store.is_null() || !store["Enable"].as_bool().unwrap_or(false){return None;}
    let name=store["Name"].as_str().or_else(||store["Path"].as_str())?;
    let values=store["Settings"].as_array().into_iter().flatten().filter_map(|setting|{
        let property=setting["Property"].as_str()?;
        let value=&setting["Value"];
        Some(format!("{{ {property}: {} }}",value.as_str().map(str::to_owned).unwrap_or_else(||value.to_string())))
    }).collect::<Vec<_>>();
    Some(if values.is_empty(){format!("'{name}'")}else{format!("'{name}: {}'",values.join(", "))})
}
pub fn format_stores(stores:&Value)->String{
    let values=stores.as_array().into_iter().flatten().filter_map(format_store).collect::<Vec<_>>();
    if values.is_empty(){"None".into()}else{values.join(", ")}
}
fn area(area:&Value)->String{format!("[{}x{}@<{}, {}>:{}°],",area["Width"],area["Height"],area["X"],area["Y"],area["Rotation"])}
pub fn getter(command:&str,row:&Value){
    let absolute=&row["AbsoluteModeSettings"];let relative=&row["RelativeModeSettings"];let bindings=&row["Bindings"];
    match command {
        "getoutputmode"=>println!("Output Mode: {}",format_store(&row["OutputMode"]).unwrap_or_else(||"None".into())),
        "getareas"=>{println!("Display area: {}",area(&absolute["Display"]));println!("Tablet area: {}",area(&absolute["Tablet"]));},
        "getsensitivity"=>{println!("Horizontal Sensitivity: {}px/mm",relative["XSensitivity"]);println!("Vertical Sensitivity: {}px/mm",relative["YSensitivity"]);println!("Relative mode rotation: {}°",relative["RelativeRotation"]);println!("Reset time: {}",relative["RelativeResetDelay"].as_str().unwrap_or(""));},
        "getbindings"=>{println!("Tip Binding: {}@{}%",format_store(&bindings["TipButton"]).unwrap_or_else(||"None".into()),bindings["TipActivationThreshold"]);println!("Pen Bindings: {}",format_stores(&bindings["PenButtons"]));println!("Express Key Bindings: {}",format_stores(&bindings["AuxButtons"]));},
        "getmiscsettings"=>{for(field,label)in[("EnableClipping","Area clipping"),("EnableAreaLimiting","Tablet area limiting"),("LockAspectRatio","Lock aspect ratio")]{println!("{label}: {}",if absolute[field].as_bool().unwrap_or(false){"True"}else{"False"});}},
        "getfilters"=>println!("Filters: {}",format_stores(&row["Filters"])),
        _=>{}
    }
}

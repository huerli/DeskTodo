//! 每个字段使用逻辑时钟与设备 UUID 决胜；删除独立版本化，不被普通编辑复活。
use crate::model::{AppData, Todo};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, HashMap};
use std::path::Path;

const FIELDS: &[&str] = &["title", "notes", "done", "due_at", "priority", "tags", "created_at", "updated_at", "completed_at", "order"];
#[derive(Debug, Clone, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Stamp { pub counter: u64, pub device: String }
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Field { pub stamp: Stamp, pub value: Value }
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Record { pub fields: BTreeMap<String, Field> }
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Document { pub schema: u32, pub clock: u64, pub records: BTreeMap<String, Record> }
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncState { pub group_id: String, pub document: Document }

pub fn device_id(dir: &Path) -> Result<String, String> {
    let path=dir.join(".sync-device-id");
    match std::fs::read_to_string(&path) {
        Ok(id) => uuid::Uuid::parse_str(id.trim()).map(|id| id.to_string()).map_err(|_| "本设备同步标识损坏".into()),
        Err(error) if error.kind()==std::io::ErrorKind::NotFound => {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
            let id=uuid::Uuid::new_v4().to_string();
            let mut options=std::fs::OpenOptions::new();
            options.write(true).create_new(true);
            match options.open(&path) {
                Ok(mut file) => { use std::io::Write; file.write_all(id.as_bytes()).map_err(|e| e.to_string())?; file.sync_all().map_err(|e| e.to_string())?; Ok(id) }
                Err(error) if error.kind()==std::io::ErrorKind::AlreadyExists => device_id(dir),
                Err(error) => Err(error.to_string()),
            }
        }
        Err(error) => Err(error.to_string()),
    }
}

impl Record {
    pub fn alive(&self) -> bool { self.fields.get("alive").and_then(|field| field.value.as_bool()).unwrap_or(false) }
    fn todo(&self,id:&str) -> Result<Todo,String> {
        let mut fields=serde_json::Map::new();
        fields.insert("id".into(),Value::String(id.into()));
        for (name,field) in &self.fields { if FIELDS.contains(&name.as_str()) { fields.insert(name.clone(),field.value.clone()); } }
        serde_json::from_value(Value::Object(fields)).map_err(|_| "服务器待办字段格式不正确".into())
    }
}

impl Document {
    fn next(&mut self,device:&str) -> Result<Stamp,String> {
        self.clock=self.clock.checked_add(1).ok_or("同步逻辑时钟超出范围")?;
        Ok(Stamp {counter:self.clock,device:device.into()})
    }
    fn put(&mut self,id:&str,name:&str,value:Value,device:&str) -> Result<(),String> {
        let unchanged=self.records.get(id).and_then(|record| record.fields.get(name)).is_some_and(|field|field.value==value);
        if unchanged { return Ok(()); }
        let stamp=self.next(device)?;
        self.records.entry(id.into()).or_default().fields.insert(name.into(),Field {stamp,value});
        Ok(())
    }
    pub fn observe(&mut self,data:&AppData,device:&str) -> Result<(),String> {
        self.schema=1;
        let active:std::collections::HashSet<_>=data.todos.iter().map(|todo|todo.id.as_str()).collect();
        let removed:Vec<_>=self.records.iter().filter(|(id,record)|record.alive()&&!active.contains(id.as_str())).map(|(id,_)|id.clone()).collect();
        for id in removed { self.put(&id,"alive",Value::Bool(false),device)?; }
        for todo in &data.todos {
            let fields=serde_json::to_value(todo).map_err(|e|e.to_string())?;
            self.put(&todo.id,"alive",Value::Bool(true),device)?;
            for name in FIELDS { self.put(&todo.id,name,fields[*name].clone(),device)?; }
        }
        for todo in &data.trash {
            if self.records.contains_key(&todo.id) { continue; }
            let fields=serde_json::to_value(todo).map_err(|e|e.to_string())?;
            self.put(&todo.id,"alive",Value::Bool(false),device)?;
            for name in FIELDS { self.put(&todo.id,name,fields[*name].clone(),device)?; }
        }
        Ok(())
    }
    pub fn merge(&mut self,other:&Self) -> Result<(),String> {
        if other.schema!=1 { return Err("服务器同步数据版本不受支持".into()); }
        self.schema=1;
        self.clock=self.clock.max(other.clock);
        for (id,record) in &other.records {
            let current=self.records.entry(id.clone()).or_default();
            for (name,incoming) in &record.fields {
                self.clock=self.clock.max(incoming.stamp.counter);
                let replace=current.fields.get(name).map_or(true,|existing| {
                    incoming.stamp>existing.stamp || (incoming.stamp==existing.stamp&&incoming.value.to_string()>existing.value.to_string())
                });
                if replace {current.fields.insert(name.clone(),incoming.clone());}
            }
        }
        Ok(())
    }
    pub fn apply(&self,data:&mut AppData) -> Result<(),String> {
        let previous:HashMap<_,_>=data.todos.iter().map(|todo|(todo.id.as_str(),todo)).collect();
        let mut active=Vec::new(); let mut trash=Vec::new();
        for (id,record) in &self.records {
            if id.is_empty() || !record.fields.get("alive").is_some_and(|field|field.value.is_boolean()) {return Err("服务器任务标识或删除状态不正确".into());}
            let mut todo=record.todo(id)?;
            if let Some(local)=previous.get(id.as_str()) { if local.due_at==todo.due_at {todo.notified_at=local.notified_at.clone();} }
            if !todo.done {todo.completed_at=None;}
            if record.alive() {active.push(todo);} else {trash.push((record.fields["alive"].stamp.clone(),todo));}
        }
        active.sort_by(|a,b|a.order.cmp(&b.order).then(a.id.cmp(&b.id)));
        trash.sort_by(|a,b|a.0.cmp(&b.0));
        data.todos=active;
        data.trash=trash.into_iter().rev().take(50).map(|(_,todo)|todo).collect();
        data.trash.reverse();
        Ok(())
    }
    /// 首次加入时不能把本机未见过的服务器任务误判为删除。
    pub fn bootstrap(data:&AppData,remote:&Self,device:&str) -> Result<Self,String> {
        let mut combined=data.clone();
        remote.apply(&mut combined)?;
        for todo in &data.todos {
            if let Some(existing)=combined.todos.iter_mut().find(|item|item.id==todo.id) {
                let updated=|todo:&Todo|chrono::DateTime::parse_from_rfc3339(&todo.updated_at).map(|date|date.timestamp_millis()).unwrap_or(i64::MIN);
                if updated(todo)>updated(existing) {*existing=todo.clone();}
            } else if !remote.records.contains_key(&todo.id) {combined.todos.push(todo.clone());}
        }
        for todo in &data.trash { if !remote.records.contains_key(&todo.id) {combined.trash.push(todo.clone());} }
        let mut document=remote.clone();
        document.observe(&combined,device)?;
        Ok(document)
    }
}

pub fn decode(plaintext:&[u8]) -> Result<Document,String> {
    let json:Value=serde_json::from_slice(plaintext).map_err(|_|"服务器同步数据格式不正确")?;
    if json.get("schema").is_some() {
        let document:Document=serde_json::from_value(json).map_err(|_|"服务器同步数据字段不正确")?;
        if document.schema!=1 {return Err("服务器同步数据版本不受支持".into());}
        // 在写回前验证所有记录，避免格式错误的数据覆盖有效快照。
        document.apply(&mut AppData::default())?;
        Ok(document)
    } else {
        let data:AppData=serde_json::from_value(json).map_err(|_|"旧版服务器备份格式不正确")?;
        let mut document=Document::default();document.observe(&data,"legacy")?;Ok(document)
    }
}

pub fn track(data:&mut AppData,dir:&Path) -> Result<(),String> {
    if let Some(mut state)=data.server_sync.take() {
        let result=device_id(dir).and_then(|device|state.document.observe(data,&device));
        data.server_sync=Some(state);
        result?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn task()->Todo {serde_json::from_str(r#"{"id":"one","title":"original","notes":"old","updated_at":"2026-10-13T00:00:00Z","order":10}"#).unwrap()}
    fn seed()->(AppData,Document) {let mut data=AppData::default();data.todos.push(task());let mut doc=Document::default();doc.observe(&data,"initial").unwrap();(data,doc)}
    #[test]
    fn concurrent_different_fields_converge() {
        let (mut a,base)=seed();let mut b=a.clone();a.todos[0].title="new title".into();b.todos[0].notes="new note".into();
        let mut left=base.clone();let mut right=base;left.observe(&a,"A").unwrap();right.observe(&b,"B").unwrap();
        let mut reverse=right.clone();reverse.merge(&left).unwrap();left.merge(&right).unwrap();
        assert_eq!(serde_json::to_string(&left).unwrap(),serde_json::to_string(&reverse).unwrap());
        left.apply(&mut a).unwrap();assert_eq!(a.todos[0].title,"new title");assert_eq!(a.todos[0].notes,"new note");
    }
    #[test]
    fn deletion_survives_stale_edit_and_trash_pruning() {
        let (mut a,base)=seed();let mut b=a.clone();a.todos.clear();let mut deleted=base.clone();deleted.observe(&a,"A").unwrap();
        b.todos[0].title="offline edit".into();let mut stale=base;stale.observe(&b,"B").unwrap();deleted.merge(&stale).unwrap();
        deleted.apply(&mut b).unwrap();assert!(b.todos.is_empty());b.trash.clear();deleted.observe(&b,"A").unwrap();deleted.merge(&stale).unwrap();
        deleted.apply(&mut b).unwrap();assert!(b.todos.is_empty());
        b.todos.push(b.trash.pop().unwrap());deleted.observe(&b,"A").unwrap();deleted.apply(&mut b).unwrap();assert_eq!(b.todos.len(),1);
    }
    #[test]
    fn empty_new_device_downloads_without_deleting_remote() {
        let (_,remote)=seed();let mut data=AppData::default();let joined=Document::bootstrap(&data,&remote,"new-device").unwrap();joined.apply(&mut data).unwrap();assert_eq!(data.todos.len(),1);
    }
    #[test]
    fn same_field_conflicts_ignore_wall_clock_skew() {
        let (mut a,base)=seed();let mut b=a.clone();
        a.todos[0].title="A value".into();a.todos[0].updated_at="2099-01-01T00:00:00Z".into();
        b.todos[0].title="B value".into();b.todos[0].updated_at="1900-01-01T00:00:00Z".into();
        let mut da=base.clone();let mut db=base;da.observe(&a,"A").unwrap();db.observe(&b,"B").unwrap();
        let mut reverse=db.clone();reverse.merge(&da).unwrap();da.merge(&db).unwrap();
        assert_eq!(serde_json::to_value(&da).unwrap(),serde_json::to_value(&reverse).unwrap());da.apply(&mut a).unwrap();assert_eq!(a.todos[0].title,"B value");
    }

}

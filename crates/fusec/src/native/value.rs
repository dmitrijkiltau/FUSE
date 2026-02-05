use std::cell::RefCell;
use std::rc::Rc;

use crate::interp::{Task, TaskResult, Value};

#[repr(u64)]
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum NativeTag {
    Int,
    Bool,
    Float,
    Null,
    Heap,
}

#[derive(Clone, Debug, PartialEq)]
pub enum HeapValue {
    String(String),
    List(Vec<NativeValue>),
    Map(std::collections::HashMap<String, NativeValue>),
    Struct {
        name: String,
        fields: std::collections::HashMap<String, NativeValue>,
    },
    Enum {
        name: String,
        variant: String,
        payload: Vec<NativeValue>,
    },
    Boxed(NativeValue),
    Task(TaskValue),
}

#[derive(Clone, Debug, PartialEq)]
pub struct TaskValue {
    pub result: TaskResultValue,
}

#[derive(Clone, Debug, PartialEq)]
pub enum TaskResultValue {
    Ok(NativeValue),
    Error(NativeValue),
    Runtime(String),
}

#[derive(Default)]
pub struct NativeHeap {
    values: Vec<HeapValue>,
}

impl NativeHeap {
    pub fn new() -> Self {
        Self { values: Vec::new() }
    }

    pub fn insert(&mut self, value: HeapValue) -> u64 {
        let idx = self.values.len() as u64;
        self.values.push(value);
        idx
    }

    pub fn get(&self, handle: u64) -> Option<&HeapValue> {
        self.values.get(handle as usize)
    }
}

#[repr(C)]
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct NativeValue {
    pub tag: NativeTag,
    pub payload: u64,
}

impl NativeValue {
    pub fn null() -> Self {
        Self {
            tag: NativeTag::Null,
            payload: 0,
        }
    }

    pub fn int(value: i64) -> Self {
        Self {
            tag: NativeTag::Int,
            payload: value as u64,
        }
    }

    pub fn bool(value: bool) -> Self {
        Self {
            tag: NativeTag::Bool,
            payload: if value { 1 } else { 0 },
        }
    }

    pub fn float(value: f64) -> Self {
        Self {
            tag: NativeTag::Float,
            payload: value.to_bits(),
        }
    }

    pub fn string(value: String, heap: &mut NativeHeap) -> Self {
        let handle = heap.insert(HeapValue::String(value));
        Self {
            tag: NativeTag::Heap,
            payload: handle,
        }
    }

    pub fn list(values: Vec<NativeValue>, heap: &mut NativeHeap) -> Self {
        let handle = heap.insert(HeapValue::List(values));
        Self {
            tag: NativeTag::Heap,
            payload: handle,
        }
    }

    pub fn map(
        values: std::collections::HashMap<String, NativeValue>,
        heap: &mut NativeHeap,
    ) -> Self {
        let handle = heap.insert(HeapValue::Map(values));
        Self {
            tag: NativeTag::Heap,
            payload: handle,
        }
    }

    pub fn struct_value(
        name: String,
        fields: std::collections::HashMap<String, NativeValue>,
        heap: &mut NativeHeap,
    ) -> Self {
        let handle = heap.insert(HeapValue::Struct { name, fields });
        Self {
            tag: NativeTag::Heap,
            payload: handle,
        }
    }

    pub fn enum_value(
        name: String,
        variant: String,
        payload: Vec<NativeValue>,
        heap: &mut NativeHeap,
    ) -> Self {
        let handle = heap.insert(HeapValue::Enum {
            name,
            variant,
            payload,
        });
        Self {
            tag: NativeTag::Heap,
            payload: handle,
        }
    }

    pub fn boxed(value: NativeValue, heap: &mut NativeHeap) -> Self {
        let handle = heap.insert(HeapValue::Boxed(value));
        Self {
            tag: NativeTag::Heap,
            payload: handle,
        }
    }

    pub fn task(result: TaskResultValue, heap: &mut NativeHeap) -> Self {
        let handle = heap.insert(HeapValue::Task(TaskValue { result }));
        Self {
            tag: NativeTag::Heap,
            payload: handle,
        }
    }

    pub fn from_value(value: &Value, heap: &mut NativeHeap) -> Option<Self> {
        match value {
            Value::Int(v) => Some(Self::int(*v)),
            Value::Bool(v) => Some(Self::bool(*v)),
            Value::Float(v) => Some(Self::float(*v)),
            Value::Null => Some(Self::null()),
            Value::String(v) => Some(Self::string(v.clone(), heap)),
            Value::List(values) => {
                let mut out = Vec::with_capacity(values.len());
                for value in values {
                    out.push(Self::from_value(value, heap)?);
                }
                Some(Self::list(out, heap))
            }
            Value::Map(values) => {
                let mut out = std::collections::HashMap::new();
                for (key, value) in values {
                    out.insert(key.clone(), Self::from_value(value, heap)?);
                }
                Some(Self::map(out, heap))
            }
            Value::Struct { name, fields } => {
                let mut out = std::collections::HashMap::new();
                for (key, value) in fields {
                    out.insert(key.clone(), Self::from_value(value, heap)?);
                }
                Some(Self::struct_value(name.clone(), out, heap))
            }
            Value::Enum {
                name,
                variant,
                payload,
            } => {
                let mut out = Vec::with_capacity(payload.len());
                for value in payload {
                    out.push(Self::from_value(value, heap)?);
                }
                Some(Self::enum_value(
                    name.clone(),
                    variant.clone(),
                    out,
                    heap,
                ))
            }
            Value::Boxed(value) => {
                let inner = value.borrow();
                let boxed = Self::from_value(&inner, heap)?;
                Some(Self::boxed(boxed, heap))
            }
            Value::Task(task) => {
                let result = match task.result_raw() {
                    TaskResult::Ok(value) => {
                        TaskResultValue::Ok(Self::from_value(&value, heap)?)
                    }
                    TaskResult::Error(value) => {
                        TaskResultValue::Error(Self::from_value(&value, heap)?)
                    }
                    TaskResult::Runtime(message) => TaskResultValue::Runtime(message),
                };
                Some(Self::task(result, heap))
            }
            _ => None,
        }
    }

    pub fn to_value(self, heap: &NativeHeap) -> Option<Value> {
        match self.tag {
            NativeTag::Int => Some(Value::Int(self.payload as i64)),
            NativeTag::Bool => Some(Value::Bool(self.payload != 0)),
            NativeTag::Float => Some(Value::Float(f64::from_bits(self.payload))),
            NativeTag::Null => Some(Value::Null),
            NativeTag::Heap => match heap.get(self.payload)? {
                HeapValue::String(value) => Some(Value::String(value.clone())),
                HeapValue::List(values) => {
                    let mut out = Vec::with_capacity(values.len());
                    for value in values {
                        out.push(value.to_value(heap)?);
                    }
                    Some(Value::List(out))
                }
                HeapValue::Map(values) => {
                    let mut out = std::collections::HashMap::new();
                    for (key, value) in values {
                        out.insert(key.clone(), value.to_value(heap)?);
                    }
                    Some(Value::Map(out))
                }
                HeapValue::Struct { name, fields } => {
                    let mut out = std::collections::HashMap::new();
                    for (key, value) in fields {
                        out.insert(key.clone(), value.to_value(heap)?);
                    }
                    Some(Value::Struct {
                        name: name.clone(),
                        fields: out,
                    })
                }
                HeapValue::Enum {
                    name,
                    variant,
                    payload,
                } => {
                    let mut out = Vec::with_capacity(payload.len());
                    for value in payload {
                        out.push(value.to_value(heap)?);
                    }
                    Some(Value::Enum {
                        name: name.clone(),
                        variant: variant.clone(),
                        payload: out,
                    })
                }
                HeapValue::Boxed(value) => {
                    let inner = value.to_value(heap)?;
                    Some(Value::Boxed(Rc::new(RefCell::new(inner))))
                }
                HeapValue::Task(task) => {
                    let result = match &task.result {
                        TaskResultValue::Ok(value) => {
                            TaskResult::Ok(value.to_value(heap)?)
                        }
                        TaskResultValue::Error(value) => {
                            TaskResult::Error(value.to_value(heap)?)
                        }
                        TaskResultValue::Runtime(message) => {
                            TaskResult::Runtime(message.clone())
                        }
                    };
                    Some(Value::Task(Task::from_task_result(result)))
                }
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_value_roundtrip_task() {
        let mut heap = NativeHeap::new();
        let task = Task::from_task_result(TaskResult::Ok(Value::Int(42)));
        let value = Value::Task(task);
        let native = NativeValue::from_value(&value, &mut heap).expect("encode failed");
        let round = native.to_value(&heap).expect("decode failed");
        match round {
            Value::Task(task) => match task.result_raw() {
                TaskResult::Ok(Value::Int(value)) => assert_eq!(value, 42),
                other => panic!("unexpected task result: {other:?}"),
            },
            other => panic!("unexpected value: {other:?}"),
        }
    }
}

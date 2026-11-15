//! Minimal safetensors reader: fp32 tensors by name.

use anyhow::{anyhow, bail, Context, Result};
use std::collections::HashMap;
use std::path::Path;

pub struct Tensors {
    map: HashMap<String, (Vec<usize>, Vec<f32>)>,
}

impl Tensors {
    pub fn open(path: &Path) -> Result<Self> {
        let bytes = std::fs::read(path).with_context(|| path.display().to_string())?;
        Self::parse(&bytes).with_context(|| path.display().to_string())
    }

    pub fn parse(bytes: &[u8]) -> Result<Self> {
        if bytes.len() < 8 { bail!("not a safetensors file") }
        let n = u64::from_le_bytes(bytes[..8].try_into().unwrap()) as usize;
        let header: serde_json::Value = serde_json::from_slice(bytes.get(8..8 + n).ok_or_else(|| anyhow!("short header"))?)?;
        let data = &bytes[8 + n..];
        let mut map = HashMap::new();
        for (name, v) in header.as_object().ok_or_else(|| anyhow!("header is not an object"))? {
            if name == "__metadata__" { continue }
            if v["dtype"] != "F32" { bail!("{name}: dtype {} (want F32)", v["dtype"]) }
            let shape: Vec<usize> = v["shape"].as_array().ok_or_else(|| anyhow!("{name}: no shape"))?
                .iter().map(|d| d.as_u64().unwrap_or(0) as usize).collect();
            let off = v["data_offsets"].as_array().ok_or_else(|| anyhow!("{name}: no offsets"))?;
            let (a, b) = (off[0].as_u64().unwrap_or(0) as usize, off[1].as_u64().unwrap_or(0) as usize);
            let raw = data.get(a..b).ok_or_else(|| anyhow!("{name}: data out of range"))?;
            if raw.len() != shape.iter().product::<usize>() * 4 { bail!("{name}: size does not match shape {shape:?}") }
            let vals = raw.chunks_exact(4).map(|c| f32::from_le_bytes(c.try_into().unwrap())).collect();
            map.insert(name.clone(), (shape, vals));
        }
        Ok(Self { map })
    }

    /// The tensor `name`, checked against `shape`.
    pub fn take(&mut self, name: &str, shape: &[usize]) -> Result<Vec<f32>> {
        let (s, v) = self.map.remove(name).ok_or_else(|| anyhow!("missing tensor {name}"))?;
        if s != shape { bail!("{name}: shape {s:?}, expected {shape:?}") }
        Ok(v)
    }

    pub fn get(&self, name: &str) -> Option<(&[usize], &[f32])> {
        self.map.get(name).map(|(s, v)| (s.as_slice(), v.as_slice()))
    }

    pub fn names(&self) -> impl Iterator<Item = &String> { self.map.keys() }
}

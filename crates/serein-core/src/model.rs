//! Immutable model pack. Missing packs explicitly select lexical fallback.
use crate::*;
use memmap2::Mmap;
use serde::Deserialize;
use std::{fs::File, path::Path};
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub format: u32,
    pub dimensions: usize,
    pub rows: usize,
    pub model_hash: String,
    pub weights_sha256: String,
    pub scales_sha256: String,
    pub tokenizer_sha256: String,
    pub special_ids: Vec<u32>,
}
pub struct Encoder {
    weights: Mmap,
    scales: Mmap,
    tokenizer: tokenizers::Tokenizer,
    pub manifest: Manifest,
}
impl Encoder {
    pub fn open(path: &Path) -> Result<Self> {
        let manifest: Manifest =
            serde_json::from_slice(&std::fs::read(path.join("manifest.json"))?)?;
        if manifest.format != 1 || manifest.dimensions != 256 || manifest.rows > 200000 {
            return Err(Error("MODEL_INVALID", "Unsupported model pack.".into()));
        }
        let wf = File::open(path.join("weights.i8"))?;
        let sf = File::open(path.join("scales.f32"))?;
        let weights = unsafe { Mmap::map(&wf)? };
        let scales = unsafe { Mmap::map(&sf)? };
        let tokens = std::fs::read(path.join("tokenizer.json"))?;
        if weights.len() != manifest.rows * 256
            || scales.len() != manifest.rows * 4
            || hash(&weights) != manifest.weights_sha256
            || hash(&scales) != manifest.scales_sha256
            || hash(&tokens) != manifest.tokenizer_sha256
        {
            return Err(Error(
                "MODEL_INVALID",
                "Model checksum mismatch. Reinstall the verified pack.".into(),
            ));
        }
        let tokenizer = tokenizers::Tokenizer::from_bytes(&tokens)
            .map_err(|_| Error("MODEL_INVALID", "Invalid tokenizer.".into()))?;
        Ok(Self {
            weights,
            scales,
            tokenizer,
            manifest,
        })
    }
    pub fn token_ids(&self, text: &str) -> Option<Vec<u32>> {
        self.tokenizer
            .encode(text, false)
            .ok()
            .map(|e| e.get_ids().to_vec())
    }
    pub fn encode(&self, text: &str) -> Option<Vec<f32>> {
        let encoding = self.tokenizer.encode(text, false).ok()?;
        let mut v = vec![0f32; 256];
        for &i in encoding.get_ids().iter().take(1024) {
            if self.manifest.special_ids.contains(&i) {
                continue;
            }
            let i = i as usize;
            if i >= self.manifest.rows {
                return None;
            }
            let scale = f32::from_le_bytes(self.scales[i * 4..i * 4 + 4].try_into().ok()?);
            if !scale.is_finite() || scale <= 0.0 {
                return None;
            }
            for (j, x) in v.iter_mut().enumerate() {
                *x += (self.weights[i * 256 + j] as i8) as f32 * scale
            }
        }
        normalize(v)
    }
}
pub fn normalize(mut v: Vec<f32>) -> Option<Vec<f32>> {
    let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm < 1e-12 || !norm.is_finite() {
        return None;
    }
    v.iter_mut().for_each(|x| *x /= norm);
    Some(v)
}
pub fn cosine(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

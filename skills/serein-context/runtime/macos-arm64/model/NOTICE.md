# Model provenance

Serein's static embedding rows and tokenizer derive from
[`sentence-transformers/static-similarity-mrl-multilingual-v1`](https://huggingface.co/sentence-transformers/static-similarity-mrl-multilingual-v1)
at immutable Hugging Face commit
`bae9c8b1d48e8962a2ce7cb207662ed2a8441ccc`.

The upstream model card declares the model under Apache License 2.0. The pinned
repository snapshot has no standalone license file, so this distribution
includes the standard license text in `LICENSE-2.0.txt`. The upstream model
card and exact source file SHA-256 values are recorded in
`docs/model-pack-report.json` and `models/manifest.json`.

The pinned model is a static multilingual similarity model trained with
Matryoshka loss. The prepared pack keeps the first 256 columns of each source
row and applies symmetric row-wise int8 quantization. The tokenizer JSON is
preserved byte for byte. Its Sentence Transformers module tokenizes with
`add_special_tokens=false` and averages all returned token rows with
`EmbeddingBag(mode="mean")`; no per-token weights are applied. Serein's
runtime sums those rows and L2-normalizes, which is cosine-equivalent to the
upstream mean pooling followed by L2 normalization. No model code, pickle,
transformer runtime, or upstream executable is included.

This model was published by the Sentence Transformers team. Its model card
describes it as a similarity model and explicitly says it is not intended for
retrieval use cases. The pack validation report tests quantization fidelity and
relative neighbor ordering against the original table; it does not make a
retrieval-quality claim.

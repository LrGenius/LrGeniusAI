"""
The NaFlex vision-tower forward shared by the SigLIP2 and AestheticSigLIP
exports.

Both are SigLIP2 So400m NaFlex encoders, and both are exported with the same
four inputs (what `lrg-ml::naflex_pre` produces):

    pixel_values          [1, N, 768] f32   patchified, normalized, padded
    pixel_attention_mask  [1, N]      i64   1 for real patches
    pos_interp_h          [gh, 16]      f32   rows of the position-embedding resize
    pos_interp_w          [gw, 16]      f32   columns of the same

`encode_vision` is transformers' `Siglip2VisionTransformer` forward with the
parts no ONNX export survives written out:

- transformers resizes the 16x16 position-embedding grid to (gh, gw) with
  `F.interpolate(..., antialias=True)`, whose output size is data dependent.
  A bilinear resize is separable, so it is `rows @ grid @ cols^T`; the caller
  supplies `rows`/`cols` (Pillow's resampling weights, the ones the image resize
  uses) and the graph does the two matmuls.
- the additive attention mask is built here instead of tracing
  `create_bidirectional_mask`.
- the attention-pooling head is written out instead of `nn.MultiheadAttention`,
  whose fast path does not export.
"""

from __future__ import annotations

POSITION_GRID = 16
PATCH_DIM = 16 * 16 * 3

# fp16 holds -1e4 and exp(-1e4) is exactly 0 in the fp32 softmax; the real
# finfo.min would overflow to -inf as soon as a score is added to it.
MASK_FILL = -1.0e4


def encode_vision(
    vision,
    pixel_values,
    pixel_attention_mask,
    pos_interp_h,
    pos_interp_w,
    tap_layers=(),
):
    """Run a transformers `Siglip2VisionTransformer` on NaFlex patches.

    Returns `(pooled, taps)`: `pooled` is the attention-pooling head's output
    `[1, D]` in the model's compute dtype, `taps` the masked-mean-pooled hidden
    state after each layer index in `tap_layers`, `[1, D]` float32 each (summed
    in fp32: a mean over hundreds of fp16 activations is not safe to do in
    fp16).
    """
    import torch
    import torch.nn.functional as F

    emb = vision.embeddings
    dtype = emb.patch_embedding.weight.dtype
    x = emb.patch_embedding(pixel_values.to(dtype))  # [1, N, D]
    n, dim = x.shape[1], x.shape[2]

    # Position embeddings: rows @ grid @ cols^T, in fp32 like transformers (it
    # upcasts for the antialiased interpolate).
    grid = emb.position_embedding.weight.reshape(POSITION_GRID, POSITION_GRID * dim)
    t = torch.matmul(pos_interp_h.float(), grid.float())  # [gh, 16*D]
    t = t.reshape(-1, POSITION_GRID, dim).permute(0, 2, 1)  # [gh, D, 16]
    pos = torch.matmul(t, pos_interp_w.float().T)  # [gh, D, gw]
    pos = pos.permute(0, 2, 1).reshape(-1, dim).to(dtype)  # [gh*gw, D]
    # Rows past gh*gw repeat row 0, as transformers pads them. They are masked
    # everywhere, so this is only for parity.
    pos = torch.cat([pos, pos[:1].expand(n, dim)], dim=0)[:n]
    h = x + pos.unsqueeze(0)

    mask = pixel_attention_mask.to(dtype)
    add_mask = ((1.0 - mask) * MASK_FILL)[:, None, None, :]  # [1, 1, 1, N]
    taps = []
    keep = mask.float()[:, :, None]  # [1, N, 1]
    real = keep.sum(dim=1).clamp(min=1.0)
    for i, layer in enumerate(vision.encoder.layers):
        h = layer(h, add_mask)
        if i in tap_layers:
            taps.append((h.float() * keep).sum(dim=1) / real)
    h = vision.post_layernorm(h)

    # Multihead attention pooling, written out.
    head = vision.head
    heads = head.num_heads
    attn = head.attention
    wq, wk, wv = attn.in_proj_weight.chunk(3, dim=0)
    bq, bk, bv = attn.in_proj_bias.chunk(3, dim=0)
    hd = dim // heads
    probe = head.probe.to(dtype)
    q = F.linear(probe, wq, bq).reshape(1, 1, heads, hd).transpose(1, 2)
    k = F.linear(h, wk, bk).reshape(1, n, heads, hd).transpose(1, 2)
    val = F.linear(h, wv, bv).reshape(1, n, heads, hd).transpose(1, 2)
    scores = torch.matmul(q, k.transpose(-1, -2)) * (hd**-0.5) + add_mask
    probs = torch.softmax(scores.float(), dim=-1).to(dtype)
    pooled = torch.matmul(probs, val).transpose(1, 2).reshape(1, 1, dim)
    pooled = attn.out_proj(pooled)
    pooled = pooled + head.mlp(head.layernorm(pooled))
    return pooled[:, 0], taps


def example_inputs(max_patches: int, gh: int, gw: int):
    """Dummy inputs for tracing, with padding so the mask path is traced."""
    import torch

    mask = torch.zeros(1, max_patches, dtype=torch.long)
    mask[0, : gh * gw] = 1
    return (
        torch.zeros(1, max_patches, PATCH_DIM),
        mask,
        torch.full((gh, POSITION_GRID), 1.0 / POSITION_GRID),
        torch.full((gw, POSITION_GRID), 1.0 / POSITION_GRID),
    )


def interp_matrix(out_size: int):
    """Rows of torch's antialiased bilinear resize 16 -> out_size, taken by
    resizing the identity: the same matrix the Rust side builds from Pillow's
    weights (checked by tests/naflex_pre_goldens.rs)."""
    import torch
    import torch.nn.functional as F

    eye = torch.eye(POSITION_GRID)[None, None]
    out = F.interpolate(eye, size=(out_size, POSITION_GRID), mode="bilinear", align_corners=False, antialias=True)
    return out[0, 0].contiguous()

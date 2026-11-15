"""Export Meta's HTDemucs weights for opendeck-demucs, and a reference run.

    uv run --python 3.11 --with demucs==4.0.1 --with torch==2.1.2 \
        --with torchaudio==2.1.2 --with 'numpy<2' --with safetensors --with soundfile \
        python tools/demucs/export_htdemucs.py OUT_DIR [track.wav]

Writes OUT_DIR/htdemucs.safetensors (every tensor of the pretrained model,
fp32, under its PyTorch name). With a track (stereo 44.1 kHz), also writes
OUT_DIR/htdemucs-ref.safetensors: one 7.8 s chunk from the middle of it and
the model's activations at each stage, in PyTorch's layout, for the crate's
layer-by-layer test (crates/demucs/tests/reference.rs).
"""
import sys, pathlib
import torch
from safetensors.torch import save_file
from demucs.pretrained import get_model

out = pathlib.Path(sys.argv[1]); out.mkdir(parents=True, exist_ok=True)
m = get_model("htdemucs").models[0].eval()
save_file({k: v.contiguous().float() for k, v in m.state_dict().items()}, out / "htdemucs.safetensors")
print("weights:", out / "htdemucs.safetensors")
if len(sys.argv) < 3:
    sys.exit(0)

import soundfile as sf
a, sr = sf.read(sys.argv[2], dtype="float32")
assert sr == 44100 and a.ndim == 2 and a.shape[1] == 2, (sr, a.shape)
n = int(m.segment * m.samplerate)
s0 = len(a) // 2
mix = torch.from_numpy(a[s0:s0 + n].T.copy())[None]

ref = {"mix": mix}
def keep(name):
    def hook(_mod, _inp, outp):
        o = outp[0] if isinstance(outp, tuple) else outp
        ref[name] = o.detach().clone()
    return hook
for i, l in enumerate(m.encoder): l.register_forward_hook(keep(f"enc{i}"))
for i, l in enumerate(m.tencoder): l.register_forward_hook(keep(f"tenc{i}"))
for i, l in enumerate(m.decoder): l.register_forward_hook(keep(f"dec{i}"))
for i, l in enumerate(m.tdecoder): l.register_forward_hook(keep(f"tdec{i}"))
for i, l in enumerate(m.crosstransformer.layers): l.register_forward_hook(keep(f"tf{i}"))
for i, l in enumerate(m.crosstransformer.layers_t): l.register_forward_hook(keep(f"tft{i}"))
m.channel_upsampler_t.register_forward_hook(keep("up_t"))
m.channel_downsampler_t.register_forward_hook(keep("down_t"))
m.crosstransformer.register_forward_pre_hook(lambda _m, inp: ref.update(tin=inp[0].clone(), tin_t=inp[1].clone()))
m.crosstransformer.register_forward_hook(lambda _m, _i, o: ref.update(tout=o[0].clone(), tout_t=o[1].clone()))
with torch.no_grad():
    z = m._spec(mix)
    ref["spec"] = torch.view_as_real(z).contiguous()
    ref["stems"] = m(mix)
save_file({k: v.contiguous().float() for k, v in ref.items()}, out / "htdemucs-ref.safetensors")
for k, v in ref.items():
    print(f"  {k:8s} {tuple(v.shape)}")

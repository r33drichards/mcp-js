"""Extract actual GPU readback evidence from cargo --nocapture logs; no drawing."""
import base64
from pathlib import Path
import sys
log = Path(sys.argv[1])
out = Path(sys.argv[2])
out.mkdir(parents=True, exist_ok=True)
for line in log.read_text().splitlines():
    if not line.startswith('GPU_FRAME_'):
        continue
    name, data = line.removeprefix('GPU_FRAME_').split(':', 1)
    if name not in ('triangle', 'sprite', 'boids'):
        raise ValueError('unexpected frame name')
    width = 256 if name == 'boids' else 64
    pixels = base64.b64decode(data, validate=True)
    assert len(pixels) == width * width * 4
    (out / (name + '.rgba')).write_bytes(pixels)
    # Pillow only packages the readback bytes; it does not generate geometry.
    from PIL import Image
    Image.frombytes('RGBA', (width, width), pixels).save(out / (name + '.png'))
    print(f'{name}: {width}x{width} GPU readback exported')

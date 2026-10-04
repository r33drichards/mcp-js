import { Buffer } from 'node:buffer';
const { PNG } = await import('npm:pngjs@7.0.0');

function samePixels(actual, expected) {
    if (actual.length !== expected.length || actual.some((byte, i) => byte !== expected[i])) {
        throw new Error('PNG pixel round trip changed the data');
    }
}

// Enough uncompressed data to exercise the bounded Inflate adapter across
// multiple native/output chunks, rather than just testing module evaluation.
const width = 97, height = 89;
const pixels = Buffer.alloc(width * height * 4);
for (let i = 0; i < pixels.length; i++) pixels[i] = (i * 37 + (i >> 7)) & 255;
const encoded = PNG.sync.write({ width, height, data: pixels }, { deflateLevel: 9 });
if (encoded[0] !== 137 || encoded[1] !== 80) throw new Error('PNG signature missing');
const decoded = PNG.sync.read(encoded);
if (decoded.width !== width || decoded.height !== height) throw new Error('PNG dimensions changed');
samePixels(decoded.data, pixels);

// pngjs's non-interlaced sync parser subclasses zlib.Inflate and calls its
// private writeSync handle. Small output buffers must not drop compressed input.
const smallPixels = Buffer.from([255, 0, 0, 255, 0, 128, 255, 64]);
const small = PNG.sync.write({ width: 2, height: 1, data: smallPixels });
samePixels(PNG.sync.read(small).data, smallPixels);

const png = new PNG({ width, height });
pixels.copy(png.data);
const asyncEncoded = await new Promise((resolve, reject) => {
    const chunks = [];
    png.on('data', chunk => chunks.push(chunk));
    png.on('end', () => resolve(Buffer.concat(chunks)));
    png.on('error', reject);
    png.pack();
});
const asyncDecoded = await new Promise((resolve, reject) => {
    new PNG().parse(asyncEncoded, (error, image) => error ? reject(error) : resolve(image));
});
samePixels(asyncDecoded.data, pixels);

const corrupted = Buffer.from(encoded);
corrupted[corrupted.length - 1] ^= 1;
let rejected = false;
try { PNG.sync.read(corrupted); } catch { rejected = true; }
if (!rejected) throw new Error('Invalid PNG CRC was accepted');

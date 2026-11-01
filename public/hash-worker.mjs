import { Sha256 } from './sha256.mjs';

// Fixed-size slices keep large APKs/videos out of main-thread memory. A single
// worker processes its requests sequentially rather than multiplying buffers.
const SLICE_BYTES = 2 * 1024 * 1024;
let pending = Promise.resolve();

async function hashFile(message) {
  const { file, id } = message ?? {};
  try {
    if (!(file instanceof Blob) || !Number.isSafeInteger(file.size)) {
      throw new TypeError('请选择可读取的原始文件');
    }
    const hash = new Sha256();
    self.postMessage({ id, progress: 0 });
    for (let offset = 0; offset < file.size; offset += SLICE_BYTES) {
      const end = Math.min(file.size, offset + SLICE_BYTES);
      const bytes = new Uint8Array(await file.slice(offset, end).arrayBuffer());
      if (bytes.length !== end - offset) throw new Error('文件读取不完整，请重新选择');
      hash.update(bytes);
      self.postMessage({ id, progress: end / file.size });
    }
    if (file.size === 0) self.postMessage({ id, progress: 1 });
    self.postMessage({ id, sha256: hash.hex() });
  } catch (error) {
    self.postMessage({ id, error: error instanceof Error ? error.message : '无法读取文件' });
  }
}

self.onmessage = event => {
  pending = pending.then(() => hashFile(event.data));
};

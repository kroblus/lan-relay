// Incremental SHA-256 (FIPS 180-4). This module does not require a secure
// browser context and retains at most one 64-byte pending block.
const INITIAL = new Uint32Array([
  0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a,
  0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19,
]);
const K = new Uint32Array([
  0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5,
  0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
  0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3,
  0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
  0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc,
  0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
  0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
  0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
  0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13,
  0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
  0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3,
  0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
  0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5,
  0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
  0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208,
  0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
]);
const rotr = (n, bits) => (n >>> bits) | (n << (32 - bits));

export class Sha256 {
  constructor() {
    this.state = INITIAL.slice();
    this.pending = new Uint8Array(64);
    this.words = new Uint32Array(64);
    this.pendingLength = 0;
    this.byteLength = 0;
  }

  update(bytes) {
    if (!(bytes instanceof Uint8Array)) throw new TypeError('SHA-256 输入必须是 Uint8Array');
    if (!Number.isSafeInteger(this.byteLength + bytes.length)) throw new RangeError('文件过大');
    this.byteLength += bytes.length;
    let offset = 0;
    if (this.pendingLength) {
      const take = Math.min(64 - this.pendingLength, bytes.length);
      this.pending.set(bytes.subarray(0, take), this.pendingLength);
      this.pendingLength += take;
      offset = take;
      if (this.pendingLength === 64) {
        this.compress(this.pending, 0);
        this.pendingLength = 0;
      }
    }
    while (offset + 64 <= bytes.length) {
      this.compress(bytes, offset);
      offset += 64;
    }
    if (offset < bytes.length) {
      this.pending.set(bytes.subarray(offset), this.pendingLength);
      this.pendingLength += bytes.length - offset;
    }
    return this;
  }

  // Return a snapshot digest. Repeated hex() calls, or later update() calls,
  // leave the previous incremental state intact.
  hex() {
    const hash = new Sha256();
    hash.state.set(this.state);
    hash.pending.set(this.pending.subarray(0, this.pendingLength));
    let length = this.pendingLength;
    hash.pending[length++] = 0x80;
    if (length > 56) {
      hash.pending.fill(0, length);
      hash.compress(hash.pending, 0);
      hash.pending.fill(0);
    } else {
      hash.pending.fill(0, length, 56);
    }
    const bitsHigh = Math.floor(this.byteLength / 0x20000000);
    const bitsLow = (this.byteLength % 0x20000000) * 8;
    const view = new DataView(hash.pending.buffer);
    view.setUint32(56, bitsHigh, false);
    view.setUint32(60, bitsLow, false);
    hash.compress(hash.pending, 0);
    return Array.from(hash.state, word => word.toString(16).padStart(8, '0')).join('');
  }

  compress(bytes, offset) {
    const w = this.words;
    for (let i = 0; i < 16; i++) {
      const p = offset + i * 4;
      w[i] = (bytes[p] << 24) | (bytes[p + 1] << 16) | (bytes[p + 2] << 8) | bytes[p + 3];
    }
    for (let i = 16; i < 64; i++) {
      const a = w[i - 15], b = w[i - 2];
      const s0 = rotr(a, 7) ^ rotr(a, 18) ^ (a >>> 3);
      const s1 = rotr(b, 17) ^ rotr(b, 19) ^ (b >>> 10);
      w[i] = (w[i - 16] + s0 + w[i - 7] + s1) >>> 0;
    }
    const state = this.state;
    let a = state[0], b = state[1], c = state[2], d = state[3];
    let e = state[4], f = state[5], g = state[6], h = state[7];
    for (let i = 0; i < 64; i++) {
      const big1 = rotr(e, 6) ^ rotr(e, 11) ^ rotr(e, 25);
      const choose = (e & f) ^ (~e & g);
      const t1 = (h + big1 + choose + K[i] + w[i]) >>> 0;
      const big0 = rotr(a, 2) ^ rotr(a, 13) ^ rotr(a, 22);
      const majority = (a & b) ^ (a & c) ^ (b & c);
      const t2 = (big0 + majority) >>> 0;
      h = g; g = f; f = e; e = (d + t1) >>> 0;
      d = c; c = b; b = a; a = (t1 + t2) >>> 0;
    }
    state[0] = (state[0] + a) >>> 0;
    state[1] = (state[1] + b) >>> 0;
    state[2] = (state[2] + c) >>> 0;
    state[3] = (state[3] + d) >>> 0;
    state[4] = (state[4] + e) >>> 0;
    state[5] = (state[5] + f) >>> 0;
    state[6] = (state[6] + g) >>> 0;
    state[7] = (state[7] + h) >>> 0;
  }
}

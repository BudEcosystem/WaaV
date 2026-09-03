/**
 * D5 uplink-shedding tests.
 *
 * A fake socket exposes a controllable `bufferedAmount` and records sent frames.
 * Asserts: the fast path sends immediately when not backpressured; under
 * backpressure (bufferedAmount over the high-water mark) frames queue; a full
 * ring sheds the OLDEST frame (bounded latency); and draining bufferedAmount
 * lets the pump flush the ring in order.
 */
import { describe, it, expect } from 'vitest';
import { UplinkAudioShedder } from '../../src/ws/uplink.js';

function frame(tag: number): ArrayBuffer {
  const b = new Uint8Array(4);
  new DataView(b.buffer).setUint32(0, tag);
  return b.buffer;
}
function tagOf(buf: ArrayBuffer): number {
  return new DataView(buf).getUint32(0);
}

class FakeSocket {
  buffered = 0;
  connected = true;
  sent: number[] = [];
  send = (f: ArrayBuffer): boolean => {
    if (!this.connected) return false;
    this.sent.push(tagOf(f));
    this.buffered += f.byteLength; // pretend it adds to the send backlog
    return true;
  };
}

describe('D5 uplink shedder', () => {
  it('fast-path sends immediately when not backpressured', () => {
    const sock = new FakeSocket();
    const shedder = new UplinkAudioShedder({
      send: sock.send,
      bufferedAmount: () => sock.buffered,
      isConnected: () => sock.connected,
    }, { highWaterBytes: 1000, maxFrames: 8 });

    shedder.push(frame(1));
    shedder.push(frame(2));
    expect(sock.sent).toEqual([1, 2]);
    expect(shedder.size()).toBe(0);
  });

  it('queues frames while bufferedAmount is over the high-water mark', () => {
    const sock = new FakeSocket();
    sock.buffered = 5000; // already backpressured
    const shedder = new UplinkAudioShedder({
      send: sock.send,
      bufferedAmount: () => sock.buffered,
      isConnected: () => sock.connected,
    }, { highWaterBytes: 1000, maxFrames: 8 });

    shedder.push(frame(1));
    shedder.push(frame(2));
    expect(sock.sent).toEqual([]); // nothing sent; socket is over the mark
    expect(shedder.size()).toBe(2);
    expect(shedder.hasPending()).toBe(true);
  });

  it('sheds the OLDEST frame when the ring is full (bounded latency)', () => {
    const sock = new FakeSocket();
    sock.buffered = 5000; // backpressured so everything queues
    const shedder = new UplinkAudioShedder({
      send: sock.send,
      bufferedAmount: () => sock.buffered,
      isConnected: () => sock.connected,
    }, { highWaterBytes: 1000, maxFrames: 3 });

    shedder.push(frame(1));
    shedder.push(frame(2));
    shedder.push(frame(3));
    shedder.push(frame(4)); // overflow -> drop oldest (1)
    shedder.push(frame(5)); // overflow -> drop oldest (2)

    expect(shedder.size()).toBe(3);
    expect(shedder.getDroppedFrames()).toBe(2);

    // Now drain: the ring holds the NEWEST three in order (3,4,5).
    sock.buffered = 0;
    shedder.pump();
    expect(sock.sent).toEqual([3, 4, 5]);
    expect(shedder.size()).toBe(0);
  });

  it('pump flushes in FIFO order once bufferedAmount drains', () => {
    const sock = new FakeSocket();
    sock.buffered = 5000; // over the mark -> everything queues
    const shedder = new UplinkAudioShedder({
      send: sock.send,
      bufferedAmount: () => sock.buffered,
      isConnected: () => sock.connected,
    }, { highWaterBytes: 1000, maxFrames: 10 });

    for (let i = 1; i <= 5; i++) shedder.push(frame(i));
    expect(sock.sent).toEqual([]); // socket backlog (5000) is over the mark (1000)

    sock.buffered = 0; // drained
    shedder.pump();
    expect(sock.sent).toEqual([1, 2, 3, 4, 5]);
  });

  it('does not send while disconnected; clear() drops the ring', () => {
    const sock = new FakeSocket();
    sock.connected = false;
    const shedder = new UplinkAudioShedder({
      send: sock.send,
      bufferedAmount: () => sock.buffered,
      isConnected: () => sock.connected,
    }, { highWaterBytes: 1000, maxFrames: 8 });

    shedder.push(frame(1));
    shedder.push(frame(2));
    expect(sock.sent).toEqual([]);
    expect(shedder.size()).toBe(2);

    shedder.clear();
    expect(shedder.size()).toBe(0);
  });
});

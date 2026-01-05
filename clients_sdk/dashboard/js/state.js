/**
 * Dashboard State Management
 */

export class State {
  constructor() {
    this._connected = false;
    this._recording = false;
    this._playing = false;
    this._transcript = '';
    this._interimTranscript = '';
    this._sttStartTime = null;
    this._sttFirstResult = false;
    this._ttsStartTime = null;
    this._ttsFirstAudio = false;
  }

  get connected() { return this._connected; }
  set connected(value) { this._connected = value; }

  get recording() { return this._recording; }
  set recording(value) { this._recording = value; }

  get playing() { return this._playing; }
  set playing(value) { this._playing = value; }

  get transcript() { return this._transcript; }
  set transcript(value) { this._transcript = value; }

  get interimTranscript() { return this._interimTranscript; }
  set interimTranscript(value) { this._interimTranscript = value; }

  get sttStartTime() { return this._sttStartTime; }
  set sttStartTime(value) {
    this._sttStartTime = value;
    this._sttFirstResult = false;
  }

  get sttFirstResult() { return this._sttFirstResult; }
  set sttFirstResult(value) { this._sttFirstResult = value; }

  get ttsStartTime() { return this._ttsStartTime; }
  set ttsStartTime(value) {
    this._ttsStartTime = value;
    this._ttsFirstAudio = false;
  }

  get ttsFirstAudio() { return this._ttsFirstAudio; }
  set ttsFirstAudio(value) { this._ttsFirstAudio = value; }

  reset() {
    this._connected = false;
    this._recording = false;
    this._playing = false;
    this._transcript = '';
    this._interimTranscript = '';
    this._sttStartTime = null;
    this._sttFirstResult = false;
    this._ttsStartTime = null;
    this._ttsFirstAudio = false;
  }
}

/**
 * Widget state machine
 */

import type { WidgetState } from './types';

export type StateTransition = {
  from: WidgetState | WidgetState[];
  to: WidgetState;
};

/**
 * Valid state transitions
 */
const validTransitions: StateTransition[] = [
  { from: 'idle', to: 'connecting' },
  { from: 'connecting', to: 'connected' },
  { from: 'connecting', to: 'error' },
  { from: 'connecting', to: 'idle' },
  { from: 'connected', to: 'listening' },
  { from: 'connected', to: 'speaking' },
  { from: 'connected', to: 'idle' },
  { from: 'connected', to: 'error' },
  { from: 'listening', to: 'connected' },
  { from: 'listening', to: 'speaking' },
  { from: 'listening', to: 'idle' },
  { from: 'listening', to: 'error' },
  { from: 'speaking', to: 'connected' },
  { from: 'speaking', to: 'listening' },
  { from: 'speaking', to: 'idle' },
  { from: 'speaking', to: 'error' },
  { from: 'error', to: 'idle' },
  { from: 'error', to: 'connecting' },
];

/**
 * State machine for widget
 */
export class StateMachine {
  private _state: WidgetState = 'idle';
  private _listeners: ((state: WidgetState, previousState: WidgetState) => void)[] = [];

  get state(): WidgetState {
    return this._state;
  }

  /**
   * Transition to a new state
   */
  transition(to: WidgetState): boolean {
    const from = this._state;

    // Check if transition is valid
    const isValid = validTransitions.some((t) => {
      const fromStates = Array.isArray(t.from) ? t.from : [t.from];
      return fromStates.includes(from) && t.to === to;
    });

    if (!isValid) {
      console.warn(`Invalid state transition: ${from} -> ${to}`);
      return false;
    }

    this._state = to;

    // Notify listeners
    for (const listener of this._listeners) {
      try {
        listener(to, from);
      } catch (e) {
        console.error('State listener error:', e);
      }
    }

    return true;
  }

  /**
   * Subscribe to state changes
   */
  subscribe(listener: (state: WidgetState, previousState: WidgetState) => void): () => void {
    this._listeners.push(listener);
    return () => {
      const index = this._listeners.indexOf(listener);
      if (index !== -1) {
        this._listeners.splice(index, 1);
      }
    };
  }

  /**
   * Check if in a specific state
   */
  is(state: WidgetState): boolean {
    return this._state === state;
  }

  /**
   * Check if in any of the specified states
   */
  isAny(...states: WidgetState[]): boolean {
    return states.includes(this._state);
  }

  /**
   * Reset to idle
   */
  reset(): void {
    this._state = 'idle';
  }
}

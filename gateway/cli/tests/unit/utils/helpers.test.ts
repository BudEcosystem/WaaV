/**
 * WaaV CLI Utility Helpers Tests
 */

import { describe, it, expect } from 'vitest';
import {
  truncate,
  capitalize,
  debounce,
  throttle,
  deepMerge,
  omit,
  pick,
  sleep,
  padString,
  centerString,
  wrapText,
  formatTimestamp,
  formatRelativeTime,
  generateId,
  deepClone,
  isPlainObject,
  getNestedValue,
  setNestedValue,
  toTitleCase,
  toKebabCase,
  toSnakeCase,
  maskSensitive,
  parseEnvVars,
  isValidUrl,
  isValidEmail,
  pluralize,
  formatNumber,
  formatPercent,
  percentile,
} from '../../../src/utils/helpers.js';

describe('sleep', () => {
  it('should wait for specified milliseconds', async () => {
    const start = Date.now();
    await sleep(50);
    const elapsed = Date.now() - start;
    expect(elapsed).toBeGreaterThanOrEqual(45); // Allow small timing variance
  });
});

describe('truncate', () => {
  it('should not truncate short strings', () => {
    expect(truncate('hello', 10)).toBe('hello');
  });

  it('should truncate long strings', () => {
    expect(truncate('hello world', 8)).toBe('hello...');
    expect(truncate('hello world', 5)).toBe('he...');
  });

  it('should use custom suffix', () => {
    expect(truncate('hello world', 8, '…')).toBe('hello w…');
  });
});

describe('padString', () => {
  it('should pad on the right by default', () => {
    expect(padString('hi', 5)).toBe('hi   ');
  });

  it('should pad on the left when specified', () => {
    expect(padString('hi', 5, ' ', 'left')).toBe('   hi');
  });

  it('should not pad if string is longer than length', () => {
    expect(padString('hello', 3)).toBe('hello');
  });
});

describe('centerString', () => {
  it('should center string within width', () => {
    expect(centerString('hi', 6)).toBe('  hi  ');
  });

  it('should handle odd padding', () => {
    expect(centerString('hi', 5)).toBe(' hi  ');
  });
});

describe('wrapText', () => {
  it('should wrap text at max width', () => {
    const result = wrapText('hello world foo bar', 10);
    expect(result).toEqual(['hello', 'world foo', 'bar']);
  });

  it('should handle long words', () => {
    const result = wrapText('superlongword short', 10);
    expect(result).toEqual(['superlongword', 'short']);
  });
});

describe('capitalize', () => {
  it('should capitalize first letter', () => {
    expect(capitalize('hello')).toBe('Hello');
  });

  it('should handle empty string', () => {
    expect(capitalize('')).toBe('');
  });

  it('should handle already capitalized', () => {
    expect(capitalize('Hello')).toBe('Hello');
  });
});

describe('toTitleCase', () => {
  it('should convert to title case', () => {
    expect(toTitleCase('hello world')).toBe('Hello World');
  });

  it('should handle underscores and hyphens', () => {
    expect(toTitleCase('hello_world-foo')).toBe('Hello World Foo');
  });
});

describe('toKebabCase', () => {
  it('should convert camelCase to kebab-case', () => {
    expect(toKebabCase('helloWorld')).toBe('hello-world');
  });

  it('should convert spaces to hyphens', () => {
    expect(toKebabCase('hello world')).toBe('hello-world');
  });
});

describe('toSnakeCase', () => {
  it('should convert camelCase to snake_case', () => {
    expect(toSnakeCase('helloWorld')).toBe('hello_world');
  });

  it('should convert spaces to underscores', () => {
    expect(toSnakeCase('hello world')).toBe('hello_world');
  });
});

describe('debounce', () => {
  it('should delay function execution', async () => {
    let callCount = 0;
    const fn = () => {
      callCount++;
    };
    const debouncedFn = debounce(fn, 50);

    debouncedFn();
    debouncedFn();
    debouncedFn();

    expect(callCount).toBe(0);

    await new Promise(resolve => setTimeout(resolve, 100));

    expect(callCount).toBe(1);
  });

  it('should pass arguments to debounced function', async () => {
    let lastArg: string | null = null;
    const fn = (arg: string) => {
      lastArg = arg;
    };
    const debouncedFn = debounce(fn, 50);

    debouncedFn('first');
    debouncedFn('second');
    debouncedFn('third');

    await new Promise(resolve => setTimeout(resolve, 100));

    expect(lastArg).toBe('third');
  });
});

describe('throttle', () => {
  it('should limit function calls', async () => {
    let callCount = 0;
    const fn = () => {
      callCount++;
    };
    const throttledFn = throttle(fn, 50);

    throttledFn();
    throttledFn();
    throttledFn();

    expect(callCount).toBe(1);

    await new Promise(resolve => setTimeout(resolve, 100));

    throttledFn();
    expect(callCount).toBe(2);
  });
});

describe('deepMerge', () => {
  it('should merge simple objects', () => {
    const target = { a: 1 };
    const source = { b: 2 };
    const result = deepMerge(target, source);

    expect(result).toEqual({ a: 1, b: 2 });
  });

  it('should merge nested objects', () => {
    const target = { a: { b: 1 } };
    const source = { a: { c: 2 } };
    const result = deepMerge(target, source);

    expect(result).toEqual({ a: { b: 1, c: 2 } });
  });

  it('should override values', () => {
    const target = { a: 1 };
    const source = { a: 2 };
    const result = deepMerge(target, source);

    expect(result).toEqual({ a: 2 });
  });

  it('should handle arrays', () => {
    const target = { arr: [1, 2] };
    const source = { arr: [3, 4] };
    const result = deepMerge(target, source);

    // Arrays are replaced, not merged
    expect(result.arr).toEqual([3, 4]);
  });
});

describe('deepClone', () => {
  it('should create a deep copy', () => {
    const original = { a: { b: 1 } };
    const clone = deepClone(original);

    expect(clone).toEqual(original);
    expect(clone).not.toBe(original);
    expect(clone.a).not.toBe(original.a);
  });
});

describe('isPlainObject', () => {
  it('should return true for plain objects', () => {
    expect(isPlainObject({})).toBe(true);
    expect(isPlainObject({ a: 1 })).toBe(true);
  });

  it('should return false for non-objects', () => {
    expect(isPlainObject(null)).toBe(false);
    expect(isPlainObject([])).toBe(false);
    expect(isPlainObject('string')).toBe(false);
    expect(isPlainObject(123)).toBe(false);
  });
});

describe('omit', () => {
  it('should omit specified keys', () => {
    const obj = { a: 1, b: 2, c: 3 };
    const result = omit(obj, ['b']);

    expect(result).toEqual({ a: 1, c: 3 });
    expect('b' in result).toBe(false);
  });

  it('should handle multiple keys', () => {
    const obj = { a: 1, b: 2, c: 3, d: 4 };
    const result = omit(obj, ['b', 'd']);

    expect(result).toEqual({ a: 1, c: 3 });
  });

  it('should handle non-existent keys', () => {
    const obj = { a: 1, b: 2 };
    const result = omit(obj, ['c' as keyof typeof obj]);

    expect(result).toEqual({ a: 1, b: 2 });
  });
});

describe('pick', () => {
  it('should pick specified keys', () => {
    const obj = { a: 1, b: 2, c: 3 };
    const result = pick(obj, ['a', 'c']);

    expect(result).toEqual({ a: 1, c: 3 });
  });

  it('should handle non-existent keys', () => {
    const obj = { a: 1, b: 2 };
    const result = pick(obj, ['a', 'c' as keyof typeof obj]);

    expect(result).toEqual({ a: 1 });
  });
});

describe('getNestedValue', () => {
  it('should get nested values using dot notation', () => {
    const obj = { a: { b: { c: 1 } } };
    expect(getNestedValue(obj, 'a.b.c')).toBe(1);
  });

  it('should return undefined for non-existent paths', () => {
    const obj = { a: 1 };
    expect(getNestedValue(obj, 'a.b.c')).toBeUndefined();
  });
});

describe('setNestedValue', () => {
  it('should set nested values using dot notation', () => {
    const obj: Record<string, unknown> = { a: {} };
    setNestedValue(obj, 'a.b.c', 1);
    expect((obj.a as Record<string, unknown>).b).toEqual({ c: 1 });
  });
});

describe('generateId', () => {
  it('should generate id of specified length', () => {
    expect(generateId(8).length).toBe(8);
    expect(generateId(16).length).toBe(16);
  });

  it('should generate unique ids', () => {
    const id1 = generateId();
    const id2 = generateId();
    expect(id1).not.toBe(id2);
  });
});

describe('formatTimestamp', () => {
  it('should format timestamp correctly', () => {
    const date = new Date('2024-01-15T10:30:45.123Z');
    expect(formatTimestamp(date)).toBe('10:30:45.123');
  });
});

describe('formatRelativeTime', () => {
  it('should format recent time as just now', () => {
    const recent = new Date(Date.now() - 30000); // 30 seconds ago
    expect(formatRelativeTime(recent)).toBe('just now');
  });

  it('should format minutes ago', () => {
    const fiveMinutesAgo = new Date(Date.now() - 5 * 60 * 1000);
    expect(formatRelativeTime(fiveMinutesAgo)).toBe('5 minutes ago');
  });

  it('should format hours ago', () => {
    const twoHoursAgo = new Date(Date.now() - 2 * 60 * 60 * 1000);
    expect(formatRelativeTime(twoHoursAgo)).toBe('2 hours ago');
  });
});

describe('maskSensitive', () => {
  it('should mask middle of string', () => {
    const result = maskSensitive('sk-1234567890abcdef');
    expect(result).toMatch(/^sk-1.*cdef$/);
    expect(result).toContain('•');
  });

  it('should mask short strings entirely', () => {
    const result = maskSensitive('short', 4);
    expect(result).toBe('•••••');
  });
});

describe('parseEnvVars', () => {
  it('should replace env var references', () => {
    process.env.TEST_VAR = 'test_value';
    const result = parseEnvVars('prefix-${TEST_VAR}-suffix');
    expect(result).toBe('prefix-test_value-suffix');
    delete process.env.TEST_VAR;
  });

  it('should replace missing vars with empty string', () => {
    const result = parseEnvVars('${NONEXISTENT_VAR}');
    expect(result).toBe('');
  });
});

describe('isValidUrl', () => {
  it('should validate correct URLs', () => {
    expect(isValidUrl('http://localhost:3001')).toBe(true);
    expect(isValidUrl('https://api.waav.io')).toBe(true);
  });

  it('should reject invalid URLs', () => {
    expect(isValidUrl('not-a-url')).toBe(false);
    expect(isValidUrl('')).toBe(false);
  });
});

describe('isValidEmail', () => {
  it('should validate correct emails', () => {
    expect(isValidEmail('test@example.com')).toBe(true);
    expect(isValidEmail('user.name@domain.org')).toBe(true);
  });

  it('should reject invalid emails', () => {
    expect(isValidEmail('not-an-email')).toBe(false);
    expect(isValidEmail('@domain.com')).toBe(false);
  });
});

describe('pluralize', () => {
  it('should return singular for count 1', () => {
    expect(pluralize('item', 1)).toBe('item');
  });

  it('should return plural for count > 1', () => {
    expect(pluralize('item', 2)).toBe('items');
    expect(pluralize('item', 0)).toBe('items');
  });

  it('should use custom plural form', () => {
    expect(pluralize('child', 2, 'children')).toBe('children');
  });
});

describe('formatNumber', () => {
  it('should format numbers with separators', () => {
    expect(formatNumber(1000)).toBe('1,000');
    expect(formatNumber(1000000)).toBe('1,000,000');
  });
});

describe('formatPercent', () => {
  it('should format percentage correctly', () => {
    expect(formatPercent(0.5)).toBe('50.0%');
    expect(formatPercent(0.123, 2)).toBe('12.30%');
  });
});

describe('percentile', () => {
  it('should calculate percentile correctly', () => {
    const values = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10];
    expect(percentile(values, 50)).toBe(5);
    expect(percentile(values, 90)).toBe(9);
  });

  it('should return 0 for empty array', () => {
    expect(percentile([], 50)).toBe(0);
  });
});

import { describe, expect, it } from 'vitest';
import { EXPECTED_API_VERSION } from './client';
import openapi from './openapi.json';

describe('EXPECTED_API_VERSION', () => {
  it('matches the version of the generated OpenAPI document', () => {
    expect(String(EXPECTED_API_VERSION)).toBe(openapi.info.version);
  });
});

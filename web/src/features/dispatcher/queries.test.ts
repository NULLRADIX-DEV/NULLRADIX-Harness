import { describe, expect, it } from 'vitest';
import { liveRegistry } from '../../app/liveRegistry';
import { dispatcherKeys } from './queries';

describe('the dispatcher live registry', () => {
  it('refreshes the configuration when its change entry or an account changes', () => {
    expect(liveRegistry['dispatcher-config']).toContainEqual(dispatcherKeys.all);
    expect(liveRegistry['account']).toContainEqual(dispatcherKeys.all);
  });
});

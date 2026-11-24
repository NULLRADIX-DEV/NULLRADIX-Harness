import { describe, expect, it } from 'vitest';
import { assetNames, baseAssetNames, missingAssets, themeAssetNames, updaterAssetNames, updaterEnabled } from '../lib/ExpectedAssets.mjs';

describe('ExpectedAssets', () => {
  it('listet die Installer und CLI-Archive je Plattform', () => {
    expect(baseAssetNames('1.2.3')).toEqual([
      'Nullradix_1.2.3_aarch64.dmg',
      'Nullradix_1.2.3_x64-setup.exe',
      'Nullradix_1.2.3_amd64.AppImage',
      'Nullradix_1.2.3_amd64.deb',
      'NULLRADIX-1.2.3-1.x86_64.rpm',
      'nullradix-cli-1.2.3-aarch64-apple-darwin.tar.gz',
      'nullradix-cli-1.2.3-x86_64-pc-windows-msvc.zip',
      'nullradix-cli-1.2.3-x86_64-unknown-linux-gnu.tar.gz',
    ]);
  });

  it('listet die zusätzlichen Updater-Dateien mit latest.json und Signaturen', () => {
    expect(updaterAssetNames('1.2.3')).toEqual([
      'latest.json',
      'Nullradix_aarch64.app.tar.gz',
      'Nullradix_aarch64.app.tar.gz.sig',
      'Nullradix_1.2.3_x64-setup.exe.sig',
      'Nullradix_1.2.3_amd64.AppImage.sig',
    ]);
  });

  it('erkennt den aktivierten Updater an bundle.createUpdaterArtifacts', () => {
    expect(updaterEnabled({ bundle: { createUpdaterArtifacts: true } })).toBe(true);
    expect(updaterEnabled({ bundle: { createUpdaterArtifacts: false } })).toBe(false);
    expect(updaterEnabled({ bundle: {} })).toBe(false);
    expect(updaterEnabled({})).toBe(false);
  });

  it('erkennt den Updater auch, wenn ihn erst die Release-Konfig aktiviert', () => {
    const basis = { bundle: { targets: ['app'] } };
    const release = { bundle: { createUpdaterArtifacts: true } };
    expect(updaterEnabled(basis, release)).toBe(true);
    expect(updaterEnabled(basis, {})).toBe(false);
    expect(updaterEnabled(basis, undefined)).toBe(false);
    expect(updaterEnabled()).toBe(false);
  });

  it('liefert ohne Updater nur die Grundliste', () => {
    expect(assetNames('1.2.3')).toEqual(baseAssetNames('1.2.3'));
    expect(assetNames('1.2.3', { updater: false })).toEqual(baseAssetNames('1.2.3'));
  });

  it('liefert mit Updater Grundliste und Updater-Dateien zusammen', () => {
    expect(assetNames('1.2.3', { updater: true })).toEqual([...baseAssetNames('1.2.3'), ...updaterAssetNames('1.2.3')]);
  });

  it('erwartet je mitgeliefertem Theme ein Paket und den Store-Index', () => {
    const themes = [{ id: 'signal', version: '1.0.0' }, { id: 'win95', version: '1.2.0' }];
    expect(themeAssetNames(themes)).toEqual(['nullradix-themes.json', 'signal-1.0.0.nullradix', 'win95-1.2.0.nullradix']);
    expect(themeAssetNames([])).toEqual([]);
    expect(assetNames('1.2.3', { themes })).toEqual([...baseAssetNames('1.2.3'), ...themeAssetNames(themes)]);
  });

  it('meldet ein fehlendes Theme-Paket', () => {
    const expected = assetNames('1.2.3', { themes: [{ id: 'signal', version: '1.0.0' }] });
    const actual = expected.filter((name) => name !== 'signal-1.0.0.nullradix');
    expect(missingAssets(expected, actual)).toEqual(['signal-1.0.0.nullradix']);
  });

  it('meldet Dateien aus expected, die in actual fehlen', () => {
    const expected = ['a', 'b', 'c'];
    expect(missingAssets(expected, ['a', 'c'])).toEqual(['b']);
    expect(missingAssets(expected, ['a', 'b', 'c'])).toEqual([]);
    expect(missingAssets(expected, [])).toEqual(expected);
  });
});

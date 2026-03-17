import { describe, expect, it } from 'vitest';
import { LanguageService, stripImportTypePrefix } from '../src/language-service';
import type { Config, ReplCliFlags } from '../src/config';

function testConfig(): Config {
  return {
    binary: 'golem-cli',
    appMainDir: '.',
    agents: {},
    historyFile: '.history',
    cliCommandsMetadataJsonPath: '.commands.json',
  };
}

function testFlags(): ReplCliFlags {
  return {
    disableAutoImports: true,
    showTypeInfo: true,
    streamLogs: false,
  };
}

describe('stripImportTypePrefix', () => {
  it('removes import(...) prefixes from type text', () => {
    expect(stripImportTypePrefix('import("pkg").Foo')).toBe('Foo');
    expect(stripImportTypePrefix('A & import("pkg").Bar')).toBe('A & Bar');
  });
});

describe('LanguageService snippet type context', () => {
  it('keeps remote method expansion consistent in quick and type info', () => {
    const service = new LanguageService(testConfig(), testFlags());
    service.setSnippet(
      'type RemoteMethod<Args, Ret> = { (args: Args): Promise<Ret>; trigger: (args: Args) => void; schedule: (scheduleAt: string, args: Args) => void }; const fn = null as any as RemoteMethod<[string], number>; fn',
    );

    const quickInfo = service.getSnippetQuickInfo();
    const typeInfo = service.getSnippetTypeInfo();

    expect(quickInfo).toBeDefined();
    expect(typeInfo).toBeDefined();
    expect(quickInfo?.formattedInfo).toContain('trigger:');
    expect(quickInfo?.formattedInfo).toContain('schedule:');
    expect(typeInfo?.formattedType).toContain('trigger:');
    expect(typeInfo?.formattedType).toContain('schedule:');
  });
});

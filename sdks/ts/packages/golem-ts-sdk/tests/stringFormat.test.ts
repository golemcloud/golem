import { convertTypeNameToKebab } from '../src/internal/mapping/types/stringFormat';
import { describe, expect } from 'vitest';

describe('convertTypeNameToKebab', () => {
  it('single word camel case', () => {
    expect(convertTypeNameToKebab('Something')).toEqual('something');
  });
  it('multi-word camel case', () => {
    expect(convertTypeNameToKebab('AnotherExample')).toEqual('another-example');
  });
  it('single-character-prefix', () => {
    expect(convertTypeNameToKebab('AAgent')).toEqual('a-agent');
  });
  // The following test cases are taken from the Rust heck library
  it('camel case with single word', () => {
    expect(convertTypeNameToKebab('CamelCase')).toEqual('camel-case');
  });
  it('human case with punctuation and spaces', () => {
    expect(convertTypeNameToKebab('This is Human case.')).toEqual('this-is-human-case');
  });
  it('mixed uppercase and spaces', () => {
    expect(convertTypeNameToKebab('MixedUP CamelCase, with some Spaces')).toEqual(
      'mixed-up-camel-case-with-some-spaces',
    );
  });
  it('snake and spaced mixed case', () => {
    expect(convertTypeNameToKebab('mixed_up_ snake_case with some _spaces')).toEqual(
      'mixed-up-snake-case-with-some-spaces',
    );
  });
  it('already kebab case', () => {
    expect(convertTypeNameToKebab('kebab-case')).toEqual('kebab-case');
  });
  it('shouty snake case', () => {
    expect(convertTypeNameToKebab('SHOUTY_SNAKE_CASE')).toEqual('shouty-snake-case');
  });
  it('simple snake case', () => {
    expect(convertTypeNameToKebab('snake_case')).toEqual('snake-case');
  });
  it('mixed boundaries and cases', () => {
    expect(convertTypeNameToKebab('this-contains_ ALLKinds OfWord_Boundaries')).toEqual(
      'this-contains-all-kinds-of-word-boundaries',
    );
  });
  it('contains greek characters', () => {
    expect(convertTypeNameToKebab('XΣXΣ baﬄe')).toEqual('xσxς-baﬄe');
  });
  it('acronym inside camel case', () => {
    expect(convertTypeNameToKebab('XMLHttpRequest')).toEqual('xml-http-request');
  });
  it('arabic script with spaces', () => {
    expect(convertTypeNameToKebab('لِنَذْهَبْ إِلَى السِّيْنَمَا')).toEqual(
      'لِنَذْهَبْ-إِلَى-السِّيْنَمَا',
    );
  });
  it('japanese unchanged', () => {
    expect(convertTypeNameToKebab('ファイルを読み込み')).toEqual('ファイルを読み込み');
  });
  it('chinese unchanged', () => {
    expect(convertTypeNameToKebab('祝你一天过得愉快')).toEqual('祝你一天过得愉快');
  });
});

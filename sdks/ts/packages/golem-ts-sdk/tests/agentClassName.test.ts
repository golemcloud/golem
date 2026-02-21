// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

import { describe, it, expect } from 'vitest';
import { AgentClassName } from '../src';

describe('AgentClassName', () => {
  describe('valid class names', () => {
    it('should accept simple class names', () => {
      expect(() => new AgentClassName('MyAgent')).not.toThrow();
      expect(() => new AgentClassName('Agent')).not.toThrow();
      expect(() => new AgentClassName('SimpleClass')).not.toThrow();
    });

    it('should accept class names with underscores', () => {
      expect(() => new AgentClassName('My_Agent')).not.toThrow();
      expect(() => new AgentClassName('User_Profile_Manager')).not.toThrow();
      expect(() => new AgentClassName('Data_Service')).not.toThrow();
    });

    it('should accept class names with dashes', () => {
      expect(() => new AgentClassName('My-Agent')).not.toThrow();
      expect(() => new AgentClassName('User-Profile-Manager')).not.toThrow();
      expect(() => new AgentClassName('Data-Service')).not.toThrow();
    });

    it('should accept class names with numbers at the end of segments', () => {
      expect(() => new AgentClassName('MyAgent2')).not.toThrow();
      expect(() => new AgentClassName('User_Profile2')).not.toThrow();
      expect(() => new AgentClassName('Data-Service123')).not.toThrow();
      expect(() => new AgentClassName('Agent_V2')).not.toThrow();
    });

    it('should accept mixed separators', () => {
      expect(() => new AgentClassName('My_Agent-Service')).not.toThrow();
      expect(() => new AgentClassName('User-Profile_Manager')).not.toThrow();
    });
  });

  describe('invalid class names', () => {
    const onlyValidChars =
      /Agent class name '.*' must contain only ASCII letters, numbers, underscores, and dashes/;
    const validSeparatorsAndSections =
      /Agent class name '.*' cannot contain consecutive underscores or dashes/;
    const notStartOrEndWithSeparator =
      /Agent class name '.*' cannot start or end with underscore or dash/;
    const noNumbersAtStart = /Agent class name '.*' segments cannot start with a number/;

    it('should reject empty or whitespace-only names', () => {
      expect(() => new AgentClassName('')).toThrow('Agent class name cannot be empty');
      expect(() => new AgentClassName('   ')).toThrow(onlyValidChars);
    });

    it('should reject names with invalid characters', () => {
      expect(() => new AgentClassName('My Agent')).toThrow(onlyValidChars);
      expect(() => new AgentClassName('My@Agent')).toThrow(onlyValidChars);
      expect(() => new AgentClassName('My.Agent')).toThrow(onlyValidChars);
      expect(() => new AgentClassName('My#Agent')).toThrow(onlyValidChars);
    });

    it('should reject names with consecutive separators', () => {
      expect(() => new AgentClassName('My__Agent')).toThrow(validSeparatorsAndSections);
      expect(() => new AgentClassName('My--Agent')).toThrow(validSeparatorsAndSections);
      expect(() => new AgentClassName('User___Profile')).toThrow(validSeparatorsAndSections);
    });

    it('should reject names starting or ending with separators', () => {
      expect(() => new AgentClassName('_MyAgent')).toThrow(notStartOrEndWithSeparator);
      expect(() => new AgentClassName('-MyAgent')).toThrow(notStartOrEndWithSeparator);
      expect(() => new AgentClassName('MyAgent_')).toThrow(notStartOrEndWithSeparator);
      expect(() => new AgentClassName('MyAgent-')).toThrow(notStartOrEndWithSeparator);
    });

    it('should reject names starting with numbers', () => {
      expect(() => new AgentClassName('2Agent')).toThrow(noNumbersAtStart);
      expect(() => new AgentClassName('123Class')).toThrow(noNumbersAtStart);
    });

    it('should reject segments starting with numbers', () => {
      expect(() => new AgentClassName('My_2Agent')).toThrow(noNumbersAtStart);
      expect(() => new AgentClassName('User-123Profile')).toThrow(noNumbersAtStart);
      expect(() => new AgentClassName('Data_9Service')).toThrow(noNumbersAtStart);
      expect(() => new AgentClassName('Agent_123_Manager')).toThrow(noNumbersAtStart);
    });
  });
});

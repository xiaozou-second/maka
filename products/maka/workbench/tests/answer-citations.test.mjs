/*
 * Licensed to the Apache Software Foundation (ASF) under one
 * or more contributor license agreements.  See the NOTICE file
 * distributed with this work for additional information
 * regarding copyright ownership.  The ASF licenses this file
 * to you under the Apache License, Version 2.0 (the
 * "License"); you may not use this file except in compliance
 * with the License.  You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing,
 * software distributed under the License is distributed on an
 * "AS IS" BASIS, WITHOUT WARRANTIES OR CONDITIONS OF ANY
 * KIND, either express or implied.  See the License for the
 * specific language governing permissions and limitations
 * under the License.
 */

import assert from 'node:assert/strict';
import test from 'node:test';
import {
  answerSource,
  citationOffsets,
  prepareCitationText,
  shouldStripCitationMarkers,
  stripCitationMarkers,
} from '../app/answerCitations.ts';
import { mergeHistoryPage } from '../app/history.ts';

const first = answerSource(
  { citation: { kind: 'url', url: 'https://www.example.org/report', title: 'Report' } },
  'first',
);
const second = answerSource(
  { citation: { kind: 'url', url: 'https://example.org/notes', title: 'Report' } },
  'second',
);
const range = (source, start, end) => ({ source, anchor: { kind: 'range', part: 0, start, end } });
const blockEnd = (source) => ({ source, anchor: { kind: 'block_end', part: 0 } });

test('range chips follow their Unicode scalar endpoints as text arrives', () => {
  const citations = [
    range(first, 0, 2),
    range(second, 1, 2),
    range(first, 1, 2),
    range(first, 2, 3),
  ];
  assert.deepEqual(citationOffsets('😀', citations), []);
  const atClaim = [
    { byte: 7, source: first },
    { byte: 7, source: second },
  ];
  assert.deepEqual(citationOffsets('😀中', citations), atClaim);
  assert.deepEqual(citationOffsets('😀中。后续', citations), [
    ...atClaim,
    { byte: 10, source: first },
  ]);
  assert.deepEqual(
    citationOffsets('😀', [range(first, 0, 0), range(first, -1, 1), range(first, 0, 2)]),
    [],
  );
});

test('a block-end chip waits for the complete history part and follows its last visible text', () => {
  const citations = [range(first, 0, 1), blockEnd(second)];
  assert.deepEqual(citationOffsets('甲', citations, 8), [{ byte: 3, source: first }]);
  assert.deepEqual(citationOffsets('甲乙\r\n', citations, 8), [
    { byte: 3, source: first },
    { byte: 6, source: second },
  ]);
  assert.deepEqual(citationOffsets('甲乙\r\n', [range(first, 0, 2), blockEnd(first)], 8), [
    { byte: 6, source: first },
  ]);
  assert.deepEqual(citationOffsets('  \n', [blockEnd(first)], 3), []);
});

test('source identity uses the URL or the document index within its request', () => {
  assert.equal(first.site, 'example.org');
  assert.equal(
    first.key,
    answerSource({ citation: { kind: 'url', url: first.url, title: 'Another title' } }, 'new').key,
  );
  assert.notEqual(first.key, second.key);
  const citation = { kind: 'document', title: 'Proceedings', documentIndex: 0 };
  const modelPart = { stepId: 'step', attemptId: 'attempt', ordinal: 1 };
  const key = answerSource({ citation, modelPart }, 'first').key;
  assert.equal(
    answerSource({ citation, modelPart: { ...modelPart, ordinal: 2 } }, 'second').key,
    key,
  );
  assert.notEqual(
    answerSource({ citation, modelPart: { ...modelPart, attemptId: 'retry' } }, 'second').key,
    key,
  );
});

test('citations survive loading the body and tail of a history source in either order', () => {
  const chunk = {
    sourceCitation: true,
    field: 'body',
    role: 'assistant',
    position: { kind: 'live', sequence: 1, fact: 1, itemIndex: 0 },
    totalBytes: 4,
    order: { path: [1], itemIndex: 0 },
  };
  const head = {
    ...chunk,
    offset: 0,
    text: 'ab',
    citation: { kind: 'url', url: first.url },
    citationAnchor: range(first, 0, 1).anchor,
  };
  const tail = { ...chunk, offset: 2, text: 'cd' };
  for (const [initial, next] of [
    [head, tail],
    [tail, head],
  ]) {
    const [previous] = mergeHistoryPage([initial], () => undefined, false, 4);
    const [merged] = mergeHistoryPage([next], () => previous, true, 3);
    assert.deepEqual(
      [merged.text, merged.citation, merged.citationAnchor],
      ['abcd', head.citation, head.citationAnchor],
    );
  }
});

test('OpenAI PUA citation markers are stripped while remapping range and block-end anchors', () => {
  const prefix = '保留 KVM 和现有磁盘。';
  const marker = '\uE200cite\uE202turn137145search0\uE201';
  const raw = `${prefix} ${marker}`;
  const rawChars = [...raw].length;
  const markerStart = [...`${prefix} `].length;
  const rawBytes = Buffer.byteLength(raw, 'utf8');
  const cleanBytes = Buffer.byteLength(prefix, 'utf8');

  const prepared = prepareCitationText(raw, [range(first, markerStart, rawChars)], rawBytes);
  assert.equal(prepared.text, prefix);
  assert.deepEqual(prepared.markers, [{ byte: cleanBytes, source: first }]);

  // Block-end anchors still validate against raw totalBytes while pointing at cleaned bytes.
  const withBlockEnd = prepareCitationText(raw, [blockEnd(second)], rawBytes);
  assert.equal(withBlockEnd.text, prefix);
  assert.deepEqual(withBlockEnd.markers, [{ byte: cleanBytes, source: second }]);
  assert.deepEqual(prepareCitationText(raw, [blockEnd(second)], rawBytes + 1).markers, []);
});

test('inline PUA markers, clusters, emoji, entities, and attributed spans preserve spacing and anchor positions', () => {
  const raw =
    '😀 Fixed in \uE200entity\uE202["software","QEMU","emulator"]\uE201 \uE200cite\uE202turn0search0\uE201 \uE200cite\uE202turn0search1\uE201 and \uE203STIMER0\uE204 \uE200cite\uE202turn0view0\uE201.';
  const scalarEndAfter = (needle) =>
    [...raw.slice(0, raw.indexOf('\uE201', raw.indexOf(needle)) + 1)].length;
  const cite1End = scalarEndAfter('turn0search0');
  const cite2End = scalarEndAfter('turn0search1');
  const cite3End = scalarEndAfter('turn0view0');

  const prepared = prepareCitationText(
    raw,
    [
      range(first, cite1End - 20, cite1End),
      range(second, cite2End - 20, cite2End),
      range(first, cite3End - 18, cite3End),
    ],
    undefined,
  );

  assert.equal(prepared.text, '😀 Fixed in QEMU and STIMER0.');
  assert.deepEqual(prepared.markers, [
    { byte: Buffer.byteLength('😀 Fixed in QEMU', 'utf8'), source: first },
    { byte: Buffer.byteLength('😀 Fixed in QEMU', 'utf8'), source: second },
    { byte: Buffer.byteLength('😀 Fixed in QEMU and STIMER0', 'utf8'), source: first },
  ]);

  assert.equal(
    stripCitationMarkers('Fixed in QEMU \uE200cite\uE202turn0search0\uE201today.'),
    'Fixed in QEMU today.',
  );
  assert.equal(
    stripCitationMarkers('  - item \uE200cite\uE202turn0search0\uE201  \n  - next'),
    '  - item  \n  - next',
  );
});

test('streaming hides incomplete PUA marker tails without flickering and only strips agent/assistant prose', () => {
  assert.equal(stripCitationMarkers('保留磁盘。 \uE200', { streaming: true }), '保留磁盘。');
  assert.equal(stripCitationMarkers('保留磁盘。 \uE200ci', { streaming: true }), '保留磁盘。');
  assert.equal(
    stripCitationMarkers('保留磁盘。 \uE200cite\uE202turn137145', { streaming: true }),
    '保留磁盘。',
  );
  assert.equal(
    stripCitationMarkers('保留磁盘。 \uE200cite\uE202turn137145', { streaming: false }),
    '保留磁盘。',
  );

  assert.equal(shouldStripCitationMarkers({ role: 'assistant', field: 'body' }), true);
  assert.equal(shouldStripCitationMarkers({ role: 'assistant', field: 'thinking' }), false);
  assert.equal(shouldStripCitationMarkers({ role: 'user', field: 'body' }), false);
  assert.equal(
    shouldStripCitationMarkers({ role: 'user', field: 'body', origin: { kind: 'session' } }),
    false,
  );
  assert.equal(
    shouldStripCitationMarkers({ role: 'user', field: 'body', origin: { kind: 'worker' } }),
    true,
  );
});

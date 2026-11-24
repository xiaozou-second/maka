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

import type { Row } from './history.ts';

export type AnswerSource = {
  key: string;
  title: string;
  url?: string;
  site?: string;
};
export type InlineCitation = {
  source: AnswerSource;
  anchor: NonNullable<Row['citationAnchor']>;
};
export type PositionedCitation = {
  byte: number;
  source: AnswerSource;
};
export type PreparedCitationText = {
  text: string;
  markers: PositionedCitation[];
};

const OPEN = '\uE200';
const CLOSE = '\uE201';
const SEP = '\uE202';
const SPAN_OPEN = '\uE203';
const SPAN_CLOSE = '\uE204';
const MARKER_CHAR = /[\uE200-\uE204]/;
const ASCII_TAG = /^[A-Za-z][A-Za-z0-9_-]*$/;
const ASCII_TOKEN = /^[A-Za-z0-9_:\-./]+$/;
const TRAILING_PUNCTUATION = /^[,.;:!?\)\]\}，。；：！？、）】》」』]$/;

type ParsedMarker =
  | { kind: 'remove'; end: number }
  | { kind: 'replace'; end: number; text: string };

export function answerSource(
  row: Pick<Row, 'citation' | 'text' | 'modelPart' | 'runId'>,
  rowId: string,
): AnswerSource | undefined {
  const citation = row.citation;
  if (!citation) return row.text ? { key: rowId, title: row.text } : undefined;
  if (citation.kind === 'document') {
    return {
      key:
        citation.documentIndex != null && row.modelPart
          ? JSON.stringify([
              'document',
              row.runId,
              row.modelPart.stepId,
              row.modelPart.attemptId,
              citation.documentIndex,
            ])
          : rowId,
      title: citation.filename ? `${citation.title} (${citation.filename})` : citation.title,
    };
  }
  return urlSource(citation.url, citation.title);
}

export function urlSource(target: string, label?: string | null): AnswerSource {
  const title = label || target;
  const url = URL.parse(target);
  return url?.protocol === 'https:' || url?.protocol === 'http:'
    ? { key: target, title, url: target, site: url.hostname.replace(/^www\./i, '') }
    : { key: target, title };
}

/** Whether a row carries model/agent prose that may contain provider PUA citation markers. */
export function shouldStripCitationMarkers(row: Pick<Row, 'role' | 'field' | 'origin'>): boolean {
  if (row.field !== 'body') return false;
  if (row.role === 'assistant') return true;
  return row.role === 'user' && Boolean(row.origin && row.origin.kind !== 'session');
}

function utf8Bytes(cp: number): number {
  return cp <= 0x7f ? 1 : cp <= 0x7ff ? 2 : cp <= 0xffff ? 3 : 4;
}

function isHorizontalSpace(ch: string): boolean {
  return ch === ' ' || ch === '\t';
}

function isSpanDelim(ch: string): boolean {
  return ch === SPAN_OPEN || ch === SPAN_CLOSE;
}

function extractEntityDisplay(payload: string): string | undefined {
  const sep = payload.indexOf(SEP);
  const jsonSlice = sep === -1 ? payload : payload.slice(0, sep);
  try {
    const parsed: unknown = JSON.parse(jsonSlice);
    if (Array.isArray(parsed) && typeof parsed[1] === 'string' && parsed[1].length > 0) {
      return parsed[1];
    }
  } catch {
    return undefined;
  }
  return undefined;
}

function classifyClosedBody(body: string, end: number): ParsedMarker | undefined {
  const sep = body.indexOf(SEP);
  if (sep !== -1) {
    const tag = body.slice(0, sep);
    if (!ASCII_TAG.test(tag)) return undefined;
    if (tag === 'entity') {
      const display = extractEntityDisplay(body.slice(sep + 1));
      if (display !== undefined) return { kind: 'replace', end, text: display };
    }
    return { kind: 'remove', end };
  }
  return ASCII_TOKEN.test(body) ? { kind: 'remove', end } : undefined;
}

function parseMarker(text: string, start: number, streaming: boolean): ParsedMarker | undefined {
  if (text[start] !== OPEN) return undefined;
  const afterOpen = start + 1;
  for (let i = afterOpen; i < text.length; i++) {
    const ch = text[i]!;
    if (ch === CLOSE) return classifyClosedBody(text.slice(afterOpen, i), i + 1);
    if (ch === OPEN || ch === '\n' || ch === '\r') return undefined;
  }
  // Reached EOF without closing U+E201.
  const tail = text.slice(afterOpen);
  const sep = tail.indexOf(SEP);
  if (streaming) {
    if (tail.length === 0 || ASCII_TAG.test(tail)) return { kind: 'remove', end: text.length };
    if (sep !== -1 && ASCII_TAG.test(tail.slice(0, sep))) {
      return { kind: 'remove', end: text.length };
    }
    return undefined;
  }
  if (sep !== -1 && ASCII_TAG.test(tail.slice(0, sep))) {
    return { kind: 'remove', end: text.length };
  }
  return undefined;
}

function skipSpanDelims(text: string, pos: number): number {
  while (pos < text.length && isSpanDelim(text[pos]!)) pos++;
  return pos;
}

function skipHorizontalSpace(text: string, pos: number): number {
  while (pos < text.length && isHorizontalSpace(text[pos]!)) pos++;
  return pos;
}

function consumeRemovedCluster(text: string, pos: number, streaming: boolean): number {
  for (;;) {
    const marker = parseMarker(text, pos, streaming);
    if (marker?.kind !== 'remove') break;
    pos = marker.end;
    const next = skipSpanDelims(text, skipHorizontalSpace(text, skipSpanDelims(text, pos)));
    if (parseMarker(text, next, streaming)?.kind === 'remove') pos = next;
    else break;
  }
  return pos;
}

/** Strip OpenAI Private Use Area citation/entity markers from prose. */
export function stripCitationMarkers(text: string, options?: { streaming?: boolean }): string {
  return prepareCitationText(text, undefined, undefined, options).text;
}

/**
 * Strip provider Private Use Area citation/entity markers and remap inline
 * citation anchors (raw Unicode scalar ranges) to UTF-8 byte offsets in the
 * cleaned Markdown text in one pass.
 */
export function prepareCitationText(
  text: string,
  citations: readonly InlineCitation[] | undefined,
  totalBytes: number | undefined,
  options?: { streaming?: boolean },
): PreparedCitationText {
  const hasMarkers = MARKER_CHAR.test(text);
  const hasCitations = Boolean(citations?.length);
  if (!hasMarkers && !hasCitations) return { text, markers: [] };

  const ranges = new Map<number, InlineCitation[]>();
  const blockEnds: InlineCitation[] = [];
  if (citations) {
    for (const citation of citations) {
      const a = citation.anchor;
      if (a.kind === 'block_end') blockEnds.push(citation);
      else if (
        Number.isInteger(a.start) &&
        Number.isInteger(a.end) &&
        a.start >= 0 &&
        a.end > a.start
      ) {
        const entries = ranges.get(a.end) ?? [];
        entries.push(citation);
        ranges.set(a.end, entries);
      }
    }
  }

  const markers = new Map<number, Map<string, AnswerSource>>();
  const insert = (at: number, group: InlineCitation[]) => {
    if (!at || !group.length) return;
    const sources = markers.get(at) ?? new Map<string, AnswerSource>();
    for (const { source } of group) sources.set(source.key, source);
    markers.set(at, sources);
  };
  const collectMarkers = (): PositionedCitation[] =>
    [...markers].flatMap(([byte, sources]) =>
      [...sources.values()].map((source) => ({ byte, source })),
    );

  if (!hasMarkers) {
    let points = 0;
    let bytes = 0;
    let lastContent = 0;
    for (let i = 0; i < text.length; ) {
      const cp = text.codePointAt(i)!;
      const step = cp > 0xffff ? 2 : 1;
      const ch = text.slice(i, i + step);
      bytes += utf8Bytes(cp);
      if (!/\s/u.test(ch)) lastContent = bytes;
      const group = ranges.get(++points);
      if (group) insert(lastContent, group);
      i += step;
    }
    if (totalBytes === undefined || totalBytes === bytes) insert(lastContent, blockEnds);
    return { text, markers: collectMarkers() };
  }

  const streaming = options?.streaming === true;
  let out = '';
  let changed = false;
  let pos = 0;
  let atLineStart = true;
  let rawPoints = 0;
  let rawBytes = 0;
  let cleanBytes = 0;
  let lastContent = 0;

  const advanceSkipped = (from: number, to: number) => {
    for (let i = from; i < to; ) {
      const cp = text.codePointAt(i)!;
      const step = cp > 0xffff ? 2 : 1;
      rawBytes += utf8Bytes(cp);
      const group = ranges.get(++rawPoints);
      if (group) insert(lastContent, group);
      i += step;
    }
  };

  const advanceEmitted = (from: number, to: number) => {
    if (from >= to) return;
    out += text.slice(from, to);
    for (let i = from; i < to; ) {
      const cp = text.codePointAt(i)!;
      const step = cp > 0xffff ? 2 : 1;
      const ch = text.slice(i, i + step);
      const b = utf8Bytes(cp);
      rawBytes += b;
      cleanBytes += b;
      if (!/\s/u.test(ch)) lastContent = cleanBytes;
      const group = ranges.get(++rawPoints);
      if (group) insert(lastContent, group);
      i += step;
    }
  };

  const emitReplacement = (display: string) => {
    out += display;
    for (let i = 0; i < display.length; ) {
      const cp = display.codePointAt(i)!;
      const step = cp > 0xffff ? 2 : 1;
      const ch = display.slice(i, i + step);
      cleanBytes += utf8Bytes(cp);
      if (!/\s/u.test(ch)) lastContent = cleanBytes;
      i += step;
    }
    if ([...display].some((c) => !isHorizontalSpace(c))) {
      atLineStart = display.endsWith('\n') || display.endsWith('\r');
    }
  };

  while (pos < text.length) {
    const ch = text[pos]!;
    if (isSpanDelim(ch)) {
      changed = true;
      advanceSkipped(pos, pos + 1);
      pos += 1;
      continue;
    }

    if (ch === OPEN) {
      const marker = parseMarker(text, pos, streaming);
      if (marker) {
        changed = true;
        if (marker.kind === 'replace') {
          emitReplacement(marker.text);
          advanceSkipped(pos, marker.end);
          pos = marker.end;
        } else {
          const clusterEnd = consumeRemovedCluster(text, pos, streaming);
          advanceSkipped(pos, clusterEnd);
          pos = clusterEnd;
        }
        continue;
      }
    }

    if (isHorizontalSpace(ch) && !atLineStart) {
      const spaceEnd = skipHorizontalSpace(text, pos);
      const afterDelims = skipSpanDelims(text, spaceEnd);
      if (parseMarker(text, afterDelims, streaming)?.kind === 'remove') {
        changed = true;
        const clusterEnd = consumeRemovedCluster(text, afterDelims, streaming);
        const afterCluster = skipSpanDelims(text, clusterEnd);
        const nextCh = afterCluster < text.length ? text[afterCluster]! : undefined;
        const dropPreSpace =
          nextCh === undefined ||
          nextCh === '\n' ||
          nextCh === '\r' ||
          isHorizontalSpace(nextCh) ||
          TRAILING_PUNCTUATION.test(nextCh);
        if (dropPreSpace) advanceSkipped(pos, spaceEnd);
        else advanceEmitted(pos, spaceEnd);
        advanceSkipped(spaceEnd, clusterEnd);
        pos = clusterEnd;
        continue;
      }
      advanceEmitted(pos, spaceEnd);
      pos = spaceEnd;
      continue;
    }

    const cp = text.codePointAt(pos)!;
    const step = cp > 0xffff ? 2 : 1;
    advanceEmitted(pos, pos + step);
    if (ch === '\n' || ch === '\r') atLineStart = true;
    else if (!isHorizontalSpace(ch)) atLineStart = false;
    pos += step;
  }

  if (totalBytes === undefined || totalBytes === rawBytes) insert(lastContent, blockEnds);
  return { text: changed ? out : text, markers: collectMarkers() };
}

/** Provider ranges count Unicode scalars. TextView anchors count original UTF-8 bytes. */
export function citationOffsets(
  text: string,
  citations: readonly InlineCitation[],
  totalBytes: number | undefined,
): PositionedCitation[] {
  return prepareCitationText(text, citations, totalBytes).markers;
}

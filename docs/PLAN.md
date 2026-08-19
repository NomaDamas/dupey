# 계획

범위가 넓어지면 파일 청소 유틸이 된다. v1은 **docx + hwpx + pdf 텍스트 + txt/md**.

## 완료 (v1)

- 워크스페이스: `dupey-core` + `dupey` CLI
- `extract`: txt/md UTF-8, 줄바꿈 정규화
- `exact_hash`: SHA-256
- `near_sig` / `score`: gaoya MinHash 128, 글자 5-gram
- CLI: `fingerprint`, `compare`, `scan`
- **docx extract** — zip + document.xml 단락 텍스트, docProps·rsId 제외, core.xml에서 내부 수정시각·revision
- **hwpx extract** — Contents/section*.xml 본문 텍스트, content.hpf에서 dc:date
- **pdf extract** — 임베디드 텍스트만(pdf-extract). 스캔본은 텍스트 없음으로 명시되고 가족 묶기에서 제외
- **LSH 가족 묶기** — exact 그룹 → near 0.90 → contains. 64×2 밴드 후보 생성 + 정확 검증, contains는 크기/Jaccard 하한 게이트 + merge intersect
- **rank** — 내부 시각(mtime보다 우선), 포함 관계, 파일명 토큰, revision, 약한 길이. 신뢰도와 이유 공개
- CLI `dupey scan DIR --json` 이 공개 계약
- live e2e (`scripts/e2e.sh`) + criterion 벤치 + 코퍼스 벤치 (`scripts/bench.sh`)

## 다음

1. hwp (바이너리) extract — 한국 사무 공백
2. pptx / xlsx extract
3. 대규모 코퍼스에서 contains 후보 생성 개선 (bottom-k 스케치)

```text
scan DIR
  -> families: [{id, files, relation: exact|near|contains}]
  -> each file: {content_hash, fuzzy, signals}
  -> pick: {ranked, reasons, confidence}
```

## 하지 말 것 (이 마일스톤)

- 임베딩
- 자동 삭제 UI
- 포맷 10개 동시 완벽
- “최신본을 항상 맞춘다” 카피

## 검증

- 한 줄 수정 제안서: near ≥ 0.85
- 무관 문서: near < 0.3
- 동일 본문, 줄바꿈만 다름: exact 동일
- docx 저장만 다시 한 복사본: extract 후 exact 동일 (extract 상륙 후)

## 공개 API (목표)

```text
extract(path) -> CanonicalText
exact_hash(text) -> sha256
near_sig(text)  -> minhash
score(sig_a, sig_b) -> 0.0~1.0
```

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
- **LSH 가족 묶기** — exact 그룹 → near 0.90 → contains. 64×2 밴드 후보 + bottom-k(k=64) 스케치 역인덱스 후보(작은 문서의 contains), 크기/Jaccard 하한 게이트 + merge intersect 검증
- **rank** — 수정 시각만 (내부 시각이 있으면 그것, 없으면 fs mtime). 파일명/포함/revision/길이는 점수에 넣지 않음
- CLI `dupey scan DIR --json` 이 공개 계약
- live e2e (`scripts/e2e.sh`) + criterion 벤치 + 코퍼스 벤치 (`scripts/bench.sh`)

- **hwp (바이너리) extract** — CFB + FileHeader 플래그 + deflate + HWPTAG_PARA_TEXT(UTF-16LE, 제어 문자 제거), \\005HwpSummaryInformation의 PIDSI_EDITTIME을 내부 수정시각으로
- **pptx extract** — 슬라이드 a:t 텍스트(슬라이드 순), 발표자 노트 제외
- **xlsx extract** — 셀 값(공유 문자열 해석), 행 단위 탭 구분, calcChain 무시, 날짜 서식 셀은 시리얼→ISO 날짜
- **contains 후보 개선** — bottom-k 스케치(k=64, df≤64 필터)
- **병렬 extract** — scan의 extract+해시+시그니처 구간을 rayon으로 병렬화 (순서 보존)

## 다음

1. 대규모(10만+) 코퍼스에서 contains 정밀도 회귀 감시
2. 스캔 pdf에 대한 OCR 브리지는 범위 밖 (문서화만)
3. hwp 표/필드 내부 텍스트 추출 여부 결정

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

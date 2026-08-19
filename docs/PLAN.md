# 계획

범위가 넓어지면 파일 청소 유틸이 된다. v1은 **docx + hwpx + pdf 텍스트 + txt/md**.

## 지금 (scaffold, 이 커밋)

- 워크스페이스: `dupey-core` + `dupey` CLI
- `extract`: txt/md UTF-8, 줄바꿈 정규화
- `exact_hash`: SHA-256
- `near_sig` / `score`: gaoya MinHash 128, 글자 5-gram
- CLI: `fingerprint`, `compare`, `scan` (scan은 포맷 라우팅만)
- 가족 클러스터 / 정본 랭킹은 타입만

## 다음

1. **docx extract** — zip + document.xml 단락 텍스트, docProps·rsId 제외
2. **hwpx extract** — 본문 텍스트. hwp는 그다음
3. **pdf extract** — 임베디드 텍스트만. 스캔본은 점수 없음으로 명시
4. **LSH 가족 묶기** — 폴더 스캔 후 exact 그룹 → near 0.90 → contains
5. **rank** — 내부 시각, 포함 관계, 파일명 토큰, 신뢰도와 이유
6. CLI `dupey scan DIR --json` 이 공개 계약

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

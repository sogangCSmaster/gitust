# 외부 전체 코드 리뷰 검토 — 2026-10-16

기준: `5f1d015` 이후 워킹 트리. 외부 보고서의 High 19개·Medium 25개·Low 8개를 실제 코드와 대조했다. 아래 번호는 원본 보고서의 번호다. 원본의 P0/P1 표시는 그대로 확정하지 않았으며, 결함의 성립과 수정 필요성을 따로 판단했다.

## High

| 번호 | 판단 | 반영 내용 |
|---|---|---|
| 1 | 수용 | linked-worktree `.git` 포인터가 common/worktrees 직계인지, admin/gitdir 역포인터와 commondir가 일치하는지 확인. bounded regular-file/no-follow 읽기. |
| 2 | 부분 수용 | 자체적으로 숨겨지는 활성 미추적 `.gitignore`를 거부. ignored 파일 전체 차단은 정상 빌드를 깨므로 적용하지 않음. 추적된 정책이 이미 제외한 빌드 디렉터리는 허용. |
| 3 | 수용 | CLI 관리 작업 승인도 clean SHA·index 숨김 플래그를 확인하고 동일 SHA를 병합 CAS에 전달. 서버 Checks 자동 실행과는 별개. |
| 4 | 수용 | 모든 Git 하위 프로세스에서 상속 GIT_* 제거. 실행형 filter/diff, askPass/sshCommand, 외부 include 및 자격증명·rewrite 설정을 공통 guard에서 거부. |
| 5 | 수용 | Smart HTTP에서 내부 refs/remotes 스테이징 ref 광고·접근 차단. |
| 6 | 수용 | fetch 출력 cap 이후 남은 stderr를 drain하여 SIGPIPE 방지. 공통 프로세스 시간·출력·자식 정리 제한 적용. |
| 7 | 수용 | CLI sandbox가 서비스 OS 락을 생성부터 실행·정리까지 보유. 취소된 blocking 정리 작업에도 락 복제본 보존. |
| 8 | 수용 | 서버 spawn이 Git 브랜치 생성부터 소유권·runtime 등록까지 repo mutation guard 보유. 취소 rollback에도 guard 유지. |
| 9 | 수용 | Developer와 MCP credential이 Merged 작업을 삭제하지 못하게 함. Maintainer/SiteAdmin의 명시적 정리는 허용하며, 발행할 승인 기록이 함께 제거됨을 문서화. |
| 10 | 수용 | 실행과 최종 Git 검사 후 상태 전이 직전에 계정 활성·저장소 접근을 재확인. 실패 시 검증 증거 없음. |
| 11 | 수용 | bounded code를 파싱한 첫 교환에서 다른 파라미터 검사보다 먼저 일회용 code 소각. |
| 12 | 수용 | 필수 ReadOnly/ReadWrite 경로가 없으면 실행 오류. 선택적 host toolchain 경로만 사전에 존재 여부 확인. |
| 13 | 수용 | bare common의 기본 .gitust 및 실제 사용자 지정 auth DB·lock·전용 디렉터리를 숨김. |
| 14 | 원문 기각 | 빈 tmpfs 또는 /dev/null 읽기가 성공한다는 사실만으로 원본 데이터 유출이 성립하지 않음. Linux bwrap의 실제 mount 동작은 이번 macOS 환경에서 실행 검증하지 못함. |
| 15 | 수용 | PendingDiskWrite가 메모리 write guard와 advisory lock을 저장·stamp 갱신·실패 rollback 완료까지 보유. |
| 16 | 수용 | live token이 있으면 초기 데이터도 live 상태로 구성. 실패·로딩 중 showcase 사용자·작업·코드를 서버 결과로 표시하지 않음. |
| 17 | 수용 | auth/me 완료 전에도 token으로 live mode 판별. 미연결 정책 제어는 demo preview로 분리하고 live에서 비활성화. |
| 18 | 수용 | 고정 CoW·latency·sandbox 성능 수치 제거. 실제 값이 없을 때 측정되지 않았음을 표시. |
| 19 | 수용 | baseline 0건을 커밋 전후 모두 강제. core·누락 패턴 포함, annotation 범위 fixture self-test 추가. 확장 탐지의 안전한 Option/default 후보도 문맥별 검토. |

## Medium

| 번호 | 판단 | 반영 내용 |
|---|---|---|
| 1 | 수용 | 등록 cap에서 기존 OAuth client를 축출하지 않고 새 등록을 429로 거부. |
| 2 | 수용 | MCP token으로 tools 목록 조회·자기 logout 허용. 일반 API 권한 확장은 없음. |
| 3 | 수용 | code를 원래 승인 session/PAT에 결속하고 교환 transaction에서 해당 credential을 다시 검증. logout/reset/revoke 후 교환 거부. |
| 4 | 수용 | 지원하지 않는 scope 거부. |
| 5 | 수용 | URI parser와 명시적 scheme·host·port 검증으로 malformed/혼동 redirect 거부. |
| 6 | 수용 | 새 서버 작업 owner를 gitust-owners에 저장·복구. Merged는 사용자 활성 quota에서 제외. 기존 기록 없는 작업은 관리자 대상으로 유지. |
| 7 | 수용 | 기본 base를 agent/*에서 선택하지 않음. 서버 spawn의 명시적 base도 outbound branch 검증. |
| 8 | 수용 | CLI discard는 선택 작업만 정리해 다른 손상 approval에 영향받지 않음. publish도 config guard 확인. |
| 9 | 수용 | fetch redirect 비활성화, credential helper/askpass 차단. |
| 10 | 수용 | cookie/key 파일·URL rewrite 등 위험 config를 공유 guard에서 거부. |
| 11 | 수용 | diff도 공통 bounded Git 프로세스 실행 경로 사용. |
| 12 | 수용 | 삭제·binary diff에서도 원래 경로를 정확히 복원. 이름의 ` b/`를 구분자로 오인하지 않음. |
| 13 | 수용 | 주 작업 트리 merge/mirror 전에 index 숨김 플래그 확인. 공통 timeout·process-group 정리 적용. |
| 14 | 수용 | sync outbound branch 유효성 강화. |
| 15 | 수용 | Smart HTTP stderr cap 이후 drain, 정상 종료 후에도 남은 프로세스 그룹 정리. |
| 16 | 수용 | reflog 첫 줄 읽기 8KiB 상한. |
| 17 | 수용 | revoked SSH key 재등록 허용. 재시작·과거 ID 반복 revoke·비활성화 뒤 재등록까지 index 정합성 검증. |
| 18 | 부분 수용 | 실패 로그인에서 불필요한 전체 rollback clone·disk transaction 제거. 작은 bucket 먼저 축출하는 방향은 이미 임계치에 도달한 bucket 보존 목적이므로 반전하지 않음. |
| 19 | 수용 | pipe drain timeout 시 task abort. scratch 삭제는 blocking pool에서 수행. |
| 20 | 부분 수용 | DB symlink/FIFO 거부, regular file 확인, 기존 파일 0600 보정. 사용자 지정 DB parent는 저장소나 공유 디렉터리일 수 있어 임의 chmod하지 않음. |
| 21 | 기각 | 서버가 1MiB 초과 시 header 검사 뒤 content:None을 반환하므로 “받은 1MiB preview를 버린다”는 전제가 틀림. 관련 클라이언트 변경 취소. |
| 22 | 수용 | sync 완료 응답을 요청 당시 token과 대조. |
| 23 | 수용 | issue 키보드 이동 시 선택 항목 scrollIntoView. |
| 24 | 수용 | 가상 로그 행의 paint clipping을 제거하고 긴 행의 가로 스크롤 보존. |
| 25 | 수용 | diff 파일 변경 시 scrollTop 초기화. auth read ordering과 mutation epoch 분리. |

## Low

| 번호 | 판단 | 반영 내용 |
|---|---|---|
| 1 | 필요한 범위 수용 | 변경한 정책 preview·OAuth 검증·세션 발급·spawn을 별도 모듈로 분리. 모든 Rust/TS/TSX 파일 600줄 미만 gate 유지. 변경 없는 임박 파일은 일괄 리팩터링하지 않음. |
| 2 | 수용 | 지적된 header·policy 스타일을 디자인 토큰으로 변경. |
| 3 | 수용 | live auth/repo client 회귀 테스트 추가. |
| 4 | 수용 | 실제 end-stream body는 deadline wrapper 생략. accepted TCP stream에 TCP_NODELAY 설정. |
| 5 | 수용 | SSH public key 16KiB 상한과 Base64 padding bit 검증. |
| 6 | 수용 | shell script 앞 `--`, toolchain canonical 경로 사용. |
| 7 | 수용 | repo 재사용·reflog base 검증·비UUID 관리 폴더 무시. |
| 8 | 수용 | target 스캔 제외, 이전 bundle 복원 시 새로 생성된 잔여 파일도 제거. |

추가로 live logout 실패를 서버 폐기 성공으로 표시하던 문제를 수정했다. 독립 Gate A에서 발견한 CLI cleanup 취소 시 서비스 락 수명 문제도 수정했다.

## 검증 범위

- Git 신규 보안 회귀 10개, DB 신규 회귀 8개, sandbox 보호/정리 회귀, 서버 신규 회귀 10개, CLI 진입점 2개, frontend auth/repo client 회귀를 포함한다.
- 서버 10개는 동일 테스트가 수정 전 기준에서 전부 실패하고 수정 후 전부 통과함을 별도 임시 build로 확인했다. DB와 Git, sandbox에서도 수정 전 실패를 확인했다.
- 실전 서버·sandbox 검증은 임시 가짜 계정·저장소·sentinel과 localhost에서만 수행했다. Linux bwrap·Windows 실제 실행, 운영 GitHub 발행은 이번 검증 범위가 아니다.
- 전체 `scripts/verify.sh` 종료 코드 0: Rust 124개·웹 38개 통과, 벤치마크 2개 기본 제외. fmt·Clippy·typecheck·번들 일치·600줄 제한·fail-open baseline 0건도 통과.
- 독립 Gate A는 Git 회귀 10개·DB 회귀 8개를 실행하고 CLI cleanup P2 수정까지 확인한 뒤, 검토 범위에서 미해결 P0/P1/P2 없음으로 CLEAR TO COMMIT을 보고했다.
- 독립 Gate B는 localhost CLI/HTTP 28개와 host Git 프로세스 경로 4개, 총 32개 실제 프로브를 통과하고 동일 범위의 CLEAR TO COMMIT을 보고했다. 임시 서버·fixture는 정리했다.
- 추가 프로세스 분리 시나리오를 검토하던 Gate A turn이 실제 도구 안전 분류 오류로 종료되어 추가 프로브 확장을 중단했다. 해당 시나리오를 다른 경로로 재시도하지 않았으며, 최종 판정은 그 전에 완료한 코드 검토와 검증 범위만 포함한다.
- Linux bwrap의 미존재 Hidden 경로가 나중에 호스트에서 생성되는 경우와 별도 세션으로 분리한 프로세스의 수명 보장은 검증하지 않았다. 이번 macOS 결과를 이 경로의 보장으로 해석하지 않는다.
- 최초 검토는 워킹 트리에서 완료했고, 이후 사용자가 상세 커밋과 푸시를 별도로 요청했다.

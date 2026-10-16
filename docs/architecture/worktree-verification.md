# 작업 검증과 승인

웹의 `Verify command`에서 명령을 입력하고 `Run verification`으로 실행한다. 종료 코드·stdout·stderr·소요 시간을 확인한 뒤 서버가 `Verified`와 검증 SHA를 반환한 작업만 `Approve & Merge`할 수 있다. 원격 발행은 이후 별도의 `Publish to GitHub` 동작이다.

## 일반 실행과 승인용 검증

HTTP `POST /api/v1/swarm/worktrees/{id}/sandbox`와 MCP `gitust_sandbox_exec` 모두 `verify` 불리언을 받는다.

```json
{"command":"cargo test --offline", "verify":true}
```

- `verify` 생략 또는 `false`: 일반 샌드박스 명령이다. 작업 파일을 편집할 수 있으며, 성공해도 `Pending`이고 승인 자격은 부여하지 않는다.
- `verify: true`: 실행 전후에 관리 작업의 branch·HEAD SHA와 clean 상태를 확인한다. 종료 코드 0, 동일한 SHA, clean 상태가 모두 확인돼야 `Verified`와 `verified_commit`을 기록한다.
- tracked 변경, staged 변경, 무시되지 않은 untracked 파일은 검증을 거부한다. `.gitignore`가 제외한 빌드 산출물은 허용한다. 검증하려는 코드와 설정은 먼저 커밋해야 한다.
- index의 `assume-unchanged`·`skip-worktree` 설정은 변경을 숨길 수 있어 검증 시 거부한다. 해당 설정을 해제한 뒤 다시 검증해야 한다. Git 상태 검사에는 호출별 10초 제한을 적용한다.
- 승인 직전에 다시 clean 상태와 SHA를 확인하고, 병합 함수에도 예상 SHA를 전달한다. 검증 이후 커밋 또는 파일이 바뀌면 승인을 거부하고 재검증을 요구한다.
- 새 명령, 실패·취소·시간 초과와 권한 검사를 통과한 Git push 시도는 이전 검증을 무효화한다. 원격 거부 또는 응답 유실 가능성이 있는 push 뒤에도 이전 `Verified`를 복원하지 않는다. 권한 단계에서 거부한 다른 사용자의 push는 작업 상태를 바꾸지 않는다.
- 승인 전 검증 결과는 메모리에만 있으며 재시작 후 `Pending`으로 돌아간다. 이미 성공한 승인·발행 기록은 별도 [영속 기록](outbound-sync.md)으로 복구한다.

## 실행 조건과 한계

HTTP 명령 실행은 SiteAdmin과 `serve --trusted-single-user-sandbox` 설정이 필요하다. MCP stdio는 로컬 사용자의 명령 실행 경로다. 현재 OS 샌드박스는 신뢰하는 단일 사용자용이며 다중 테넌트의 격리 경계로 제공하지 않는다. 실행 중 네트워크는 차단하므로 필요한 도구와 의존성은 미리 준비해야 한다.

수동 검증 명령의 의미는 운영자가 결정한다. `true` 같은 명령도 성공하므로 `Verified`가 특정 테스트 모음을 통과했다는 뜻은 아니다. 저장소 설정에 따른 필수 Checks와 자동 CI는 아직 구현하지 않았다. 검증 중 다른 프로세스가 파일을 임시 변경했다가 되돌리는 행위까지 증명하는 불변 실행 스냅샷도 아니다. 동일 호스트에서 작업을 동시에 직접 수정하지 않아야 한다.

CLI `triage approve`는 로컬 운영자의 명시적 승인 경로이며 서버의 검증 기록을 읽지 않는다. CLI를 사용할 때는 운영자가 해당 커밋의 검증을 별도로 마쳐야 한다.

## 회귀 검증

`tests_verification.rs`는 검증 후 새 커밋, 검증 명령의 파일 변경, 실행 전 untracked 파일, 실패한 재실행, 승인 직전 dirty 상태, 재시작 후 검증 무효화, index 숨김 설정을 검사한다. 최초 두 사례와 index 설정 사례는 수정 전 성공 응답으로 실패했고 수정 후 거부된다. 일반 MCP 편집 명령과 실제 HTTP Git push의 상태 전이도 서버 테스트에 포함한다.

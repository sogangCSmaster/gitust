# 승인된 커밋의 Outbound 동기화

에이전트 브랜치를 내부에서 검토·병합한 다음, 사람이 명시적으로 선택한 커밋을 외부 저장소의 기본 브랜치로 전송한다. 자동 전송은 기본 동작이 아니다. CLI·HTTP API·웹 화면에서 승인과 발행을 분리하며, 백그라운드 재시도 큐는 포함하지 않는다.

## 웹 화면

실서버에 연결한 Agent Swarm에서 `Approve & Merge`로 검증된 작업을 로컬 병합하고, `Publish to GitHub`로 승인한 SHA를 서버에 설정된 원격으로 전송한다. 전송 성공 응답을 받은 뒤 동기화 상태를 표시하며, 실패하면 같은 작업에서 재시도할 수 있다. 오래된 목록 응답이나 계정 변경 전 요청의 응답은 진행 중인 작업과 현재 계정 상태를 덮어쓰지 않는다.

## CLI

```sh
gitust triage approve agent/claude/task-1 --repo /srv/gitust/repo \
  --base main --push-to git@github.com:example/project.git
```

`--push-to`를 생략하면 기존처럼 로컬 병합만 한다. 지정하면 fast-forward 병합으로 확정한 SHA를 `refs/heads/main`에 전송한다. 원격 충돌·통신 실패가 발생하면 종료 코드가 실패이며 로컬 병합은 보존된다. 오류에 표시된 SHA를 검토한 다음 아래 명령으로 재전송한다.

```sh
gitust triage publish --repo /srv/gitust/repo --base main \
  --approved-commit <승인한-40자리-SHA> --to git@github.com:example/project.git
```

이 명령은 해당 SHA의 외부 공개를 운영자가 명시적으로 승인하는 명령이다. SHA는 지정한 로컬 기본 브랜치에 포함되어야 하며 `HEAD`나 브랜치 이름은 받을 수 없다. 같은 기본 브랜치와 SHA를 가진 관리 작업의 승인 기록이 하나 있으면 그 발행 기록도 갱신한다. 일치하는 기록이 여러 개면 임의로 고르지 않고 실패하므로, 서버 API에서 작업 ID를 지정해 발행한다. 실행 중인 서버가 저장소 잠금을 보유하면 CLI는 실패하므로, 서버 운영 중에는 아래 API를 사용한다.

## HTTP API

승인 전에 웹 `Verify command` 또는 `verify: true`인 sandbox 요청으로 해당 커밋을 검증해야 한다. 일반 명령 실행 성공만으로는 승인할 수 없다. 자세한 계약은 [작업 검증과 승인](worktree-verification.md)을 참고한다.

기존 `POST /api/v1/swarm/worktrees/{id}/approve`로 검증된 작업을 병합한 뒤, 별도로 `POST /api/v1/swarm/worktrees/{id}/publish`를 호출한다. 본문은 비어 있거나 `{}`여야 한다. URL·SHA·브랜치를 본문으로 덮어쓸 수 없다. 전송 대상은 서버의 `--sync-github` 설정이다.

- 로그인 세션 또는 PAT를 가진 Maintainer/SiteAdmin과 해당 저장소 접근 권한이 필요하다. MCP 전용 OAuth 토큰은 승인·전송을 포함한 일반 API와 Git HTTP 인증에 사용할 수 없다.
- `Merged` 상태의 작업만 허용하며, 승인 때 기록한 SHA를 전송한다. 승인 뒤 로컬 `main`에 추가된 커밋은 전송하지 않는다.
- 성공 후에만 `github_synced=true`가 된다. 실패 시 승인 상태를 보존한다. 동일 API로 동일 SHA를 재시도할 수 있다.
- 전송 중에도 로컬 저장소 전체의 mutation lock은 잡지 않는다. 전송 잠금과 해당 작업의 실행 잠금은 실제 전송 및 상태 갱신까지 유지한다.
- 승인과 발행 기록은 디스크에 저장하고 재시작 때 검증하여 복구한다. 원격 설정이 바뀌면 이전 원격의 발행을 새 원격의 동기화 완료로 표시하지 않는다.

## 승인 기록과 재시작 복구

관리 워크트리의 승인 기록은 Git common directory의 `gitust-approvals/<worktree UUID>.json`에 저장한다. 작업·브랜치·기본 브랜치·승인 SHA와 마지막 발행 대상을 기록하며, 임시 파일 쓰기와 파일 동기화, 원자 교체, 디렉터리 동기화가 끝나야 성공을 반환한다. 백업에는 이 common directory와 관리 워크트리를 함께 포함해야 한다.

복구 때 파일 크기·버전·작업 ID·브랜치와 승인 SHA의 기본 브랜치 포함 여부를 검증한다. 손상된 기록, symlink, 복구 한도 초과는 서버 시작 오류로 전파한다. 기록이 없는 작업의 승인을 추정하지 않는다. 기존 버전에서 승인했지만 기록이 없는 작업은 서버를 중지하고 SHA를 검토한 뒤 위 CLI로 명시적으로 발행할 수 있다.

Git 병합과 승인 파일 저장은 하나의 트랜잭션이 아니다. 병합 직후 저장 오류나 비정상 종료가 발생하면 로컬 병합만 남을 수 있다. API 오류 또는 웹 화면 복원은 Git 병합을 되돌렸다는 뜻이 아니므로 저장소 상태를 확인해야 한다. 승인 기록 없이 남은 병합은 사람이 SHA를 다시 확인하고 CLI로 복구한다. 원격 전송 뒤 발행 기록 저장만 실패한 경우에도 실패를 반환하며, 동일 SHA를 재전송하면 기록까지 안전하게 갱신할 수 있다.

## 전송 및 인증 경계

하나의 정확한 `SHA:refs/heads/<base>`만 일반 fast-forward push로 전송한다. `agent/*`, 태그, 다른 브랜치, 승인 SHA에 도달할 수 없는 실험 커밋은 전송하지 않는다. 승인 SHA의 조상 커밋과 트리는 Git 이력의 일부로 함께 전달된다.

전송용 임시 bare 저장소는 원본의 객체만 읽는다. 원본의 remote 설정, `pushInsteadOf`, `push.followTags`, hooks, Git 환경 변수는 전송 동작에 영향을 줄 수 없다. 프로세스 시간 제한과 출력 drain을 적용한다. 시간 초과나 연결 종료 뒤에는 원격 반영 여부가 불명확할 수 있으므로 같은 승인 SHA로 재시도한다. 원격 이력을 강제로 덮어쓰지 않는다.

GitHub 인증은 실행 계정의 SSH 키/SSH agent와 사전에 확인한 `known_hosts`를 사용한다. HTTPS credential helper와 URL 내 토큰은 사용하지 않으므로, 인증이 필요한 GitHub 저장소에는 SSH URL을 설정해야 한다. 로컬 bare 저장소를 대상으로도 사용할 수 있다.

세션 DB에는 `Login`/`Mcp` 용도를 저장한다. 이전 버전의 용도 없는 세션은 안전하게 구분할 수 없어 거부되며, 업그레이드 후 웹 재로그인과 MCP OAuth 재연결이 필요하다. 사용자 계정·PAT·SSH 키는 유지된다.

## 검증

`scripts/verify.sh`에서 Outbound의 정확한 SHA/refs, 미승인 및 MCP 토큰 거부, 요청 본문 중 토큰 취소, 원격 충돌·실패 보존, 명시적 CLI 재시도, 승인·발행의 재시작 복구, 손상 기록 거부, 세션 용도 영속화 및 이전 세션 거부를 검사한다. 웹 클라이언트의 발행·실패·재시도·계정 변경·오래된 응답 처리도 포함한다. 전송 테스트는 임시 로컬 저장소만 사용하며 실제 GitHub 인증이나 운영 배포를 확인하는 테스트는 아니다.

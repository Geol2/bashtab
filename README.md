# bashtab

Git Bash 전용 탭 터미널 (Windows).

Git for Windows에 들어 있는 **진짜 mintty 창**을 탭 안에 그대로 끼워 넣습니다.
터미널을 새로 구현하지 않기 때문에 글꼴·색·`~/.minttyrc` 설정이 평소 Git Bash와 똑같고,
exe 하나(약 300KB)에 런타임도 필요 없습니다.

## 특징

- 탭마다 실제 mintty + bash 프로세스 (Win32 `SetParent`로 임베드)
- **WSL 탭**: `+` 옆 `▾` 메뉴에서 설치된 WSL 배포판(Ubuntu 등)을 골라 같은 창에서 열기
  - 탭 앞 점 색으로 구분: 주황 = Git Bash, 보라 = WSL
  - WSL이 오류로 시작에 실패하면 탭을 남겨 오류 메시지를 보여줌
- 가벼운 시작 스크립트: `/etc/profile` 대신 외부 프로세스를 띄우지 않는 [`fast-rc.sh`](fast-rc.sh) 사용
  - Git Bash와 같은 프롬프트(git 브랜치 표시 포함), git 자동완성(첫 Tab에 로드), `~/.bashrc` 유지
- 다크 탭 바, 고해상도(DPI) 대응

## 단축키

| 키 | 동작 |
|---|---|
| Ctrl+Shift+T / `+` / 탭 바 빈 곳 더블클릭 | 새 Git Bash 탭 |
| Ctrl+Shift+L | 새 WSL 탭 (기본 배포판) |
| `▾` / `+` 우클릭 | 프로필 선택 메뉴 (Git Bash / WSL 배포판) |
| Ctrl+Shift+W / 휠클릭 / × | 탭 닫기 |
| Ctrl+Tab / Ctrl+Shift+Tab | 다음 / 이전 탭 |
| Alt+1~9 | N번째 탭 (9 = 마지막) |
| Ctrl+Shift+C / Ctrl+Shift+V | 복사 / 붙여넣기 (mintty) |

## 설치

[Git for Windows](https://gitforwindows.org/)가 설치되어 있어야 합니다.
[Releases](https://github.com/Geol2/bashtab/releases)에서 `bashtab.exe`를 받아 실행하면 끝입니다.

## 빌드

```bash
cargo build --release   # target/release/bashtab.exe
```

`BASHTAB_LOG=1` 환경 변수를 주고 실행하면 `%LOCALAPPDATA%\bashtab\debug.log`에 디버그 로그를 남깁니다.

## 아이콘

`icon.ico` / `icon.png`는 [`tools/make-icon.ps1`](tools/make-icon.ps1)로 생성합니다.

```powershell
./tools/make-icon.ps1 .
```

bashtab은 Git for Windows 프로젝트와 관련이 없습니다.

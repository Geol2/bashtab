# bashtab

Git Bash 전용 탭 터미널 (Windows).

Git for Windows에 들어 있는 **진짜 mintty 창**을 탭 안에 그대로 끼워 넣습니다.
터미널을 새로 구현하지 않기 때문에 글꼴·색·`~/.minttyrc` 설정이 평소 Git Bash와 똑같고,
exe 하나(약 300KB)에 런타임도 필요 없습니다.

## 특징

- 탭마다 실제 mintty + bash 프로세스 (Win32 `SetParent`로 임베드)
- **크롬 스타일 탭 바**: 제목표시줄 자리에 탭이 들어가고, 최소화·최대화·닫기 버튼을 직접 그림
  - 빈 곳을 끌어 창 이동, 더블클릭으로 최대화, Win11 스냅 레이아웃(최대화 버튼에 마우스 올리기) 지원
- **홈 화면**: 처음 켜거나 `+`를 누르면 열리는 새 탭 화면
  - 셸 카드(Git Bash / WSL 배포판)를 눌러 그 탭에서 바로 시작
  - 즐겨찾는 폴더: 폴더를 끌어다 놓거나 "+ 폴더 추가"로 등록, 클릭하면 그 폴더에서 Git Bash, 우클릭하면 WSL로 열기 / 제거
  - **글꼴**: 설치된 고정폭 글꼴과 크기를 골라 터미널 글꼴로 사용 (기본 Consolas 11pt, 새 탭부터 적용)
    - "mintty 기본 설정 사용"을 고르면 `~/.minttyrc` 설정을 그대로 씀
  - 단축키 안내
- **WSL 탭**: 홈 화면이나 `+` 옆 `▾` 메뉴에서 설치된 WSL 배포판(Ubuntu 등)을 골라 같은 창에서 열기
  - 탭 앞 점 색으로 구분: 주황 = Git Bash, 보라 = WSL
  - WSL이 오류로 시작에 실패하면 탭을 남겨 오류 메시지를 보여줌
  - 배포판 디스크(`ext4.vhdx`)의 소유자가 Administrators라서 `E_ACCESSDENIED`로 못 켜지는 경우(관리자 터미널에서 `wsl --import`/`--move` 했을 때)를 미리 감지해, UAC 확인 한 번으로 소유자를 현재 사용자로 바꿔 줌
- 가벼운 시작 스크립트: `/etc/profile` 대신 외부 프로세스를 띄우지 않는 [`fast-rc.sh`](fast-rc.sh) 사용
  - Git Bash와 같은 프롬프트(git 브랜치 표시 포함), git 자동완성(첫 Tab에 로드), `~/.bashrc` 유지
- 다크 탭 바, 고해상도(DPI) 대응

## 단축키

| 키 | 동작 |
|---|---|
| `+` | 새 탭 (홈 화면) |
| Ctrl+Shift+T | 새 Git Bash 탭 |
| Ctrl+Shift+L | 새 WSL 탭 (기본 배포판) |
| `▾` / `+` 우클릭 | 셸 선택 메뉴 (홈 / Git Bash / WSL 배포판) |
| Ctrl+Shift+W / 휠클릭 / × | 탭 닫기 |
| 탭 우클릭 | 다시 시작 / 강제 종료 / 탭 닫기 |
| Ctrl+Tab / Ctrl+Shift+Tab | 다음 / 이전 탭 |
| Alt+1~9 | N번째 탭 (9 = 마지막) |
| Ctrl+Shift+C / Ctrl+Shift+V | 복사 / 붙여넣기 (mintty) |

## 탭이 멈춘 것 같을 때

java, node, python 같은 Windows 프로그램을 앞쪽(foreground)에서 실행하면, 그 프로그램이 끝날 때까지 프롬프트가 돌아오지 않습니다.
mintty에서는 이런 프로그램의 출력이 보이지 않거나 Ctrl+C가 안 먹을 수도 있습니다.

- **탭 우클릭 → 강제 종료**: 그 탭에서 띄운 프로그램까지 모두 종료합니다 (탭마다 Windows 작업 개체로 묶어 관리).
- **탭 우클릭 → 다시 시작**: 모두 종료하고 같은 셸을 새로 엽니다.
- 서버나 에이전트처럼 계속 떠 있어야 하는 프로그램은 `winpty java ...`로 실행하거나, 서비스·백그라운드(`... > log.txt 2>&1 &`)로 돌리는 것을 권장합니다.

창을 닫으면 각 탭에 정상 종료를 요청합니다(bash가 남지 않음). 실행 중인 프로그램이 있어 mintty가 확인을 묻는 동안 한 번 더 닫으면 강제로 모두 종료합니다.

## 설치

[Git for Windows](https://gitforwindows.org/)가 설치되어 있어야 합니다 (설치 프로그램 또는 `scoop install git`).

**Scoop (권장, SmartScreen 경고 없음)**

```powershell
scoop bucket add bashtab https://github.com/Geol2/bashtab
scoop install bashtab/bashtab
```

**직접 다운로드**

[Releases](https://github.com/Geol2/bashtab/releases)에서 `bashtab.exe`를 받아 실행하면 끝입니다.
아직 코드 서명이 없어서 처음 실행할 때 SmartScreen 경고가 뜨면 "추가 정보 → 실행"을 누르세요.

## 빌드

```bash
cargo build --release   # target/release/bashtab.exe
```

`v*` 태그를 push하면 GitHub Actions가 exe를 빌드해 Release에 올리고, Scoop manifest(`bucket/bashtab.json`)의 버전과 해시를 갱신합니다.

```bash
git tag v0.5.2 && git push origin v0.5.2
```

`BASHTAB_LOG=1` 환경 변수를 주고 실행하면 `%LOCALAPPDATA%\bashtab\debug.log`에 디버그 로그를 남깁니다.
즐겨찾는 폴더는 `%LOCALAPPDATA%\bashtab\favorites.txt`에 한 줄에 하나씩, 글꼴 설정은 `settings.ini`(`font=`, `font_size=`)에 저장됩니다.

## 아이콘

`icon.ico` / `icon.png`는 [`tools/make-icon.ps1`](tools/make-icon.ps1)로 생성합니다.

```powershell
./tools/make-icon.ps1 .
```

bashtab은 Git for Windows 프로젝트와 관련이 없습니다.

## 라이선스

[MIT](LICENSE)

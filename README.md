# Git Bash Tabs

Git Bash 전용 탭 터미널 (Windows). Electron + xterm.js + node-pty.

## 특징

- Git Bash만 띄우는 탭 터미널
- 빠른 시작: `/etc/profile` 대신 외부 프로세스를 하나도 띄우지 않는 [`fast-rc.sh`](fast-rc.sh) 사용
  - 백신/보안 프로그램 때문에 프로세스 생성이 느린 PC에서 수십 초 → 1초 이내
  - Git Bash와 같은 프롬프트(git 브랜치 표시 포함), git 자동완성(첫 Tab에 로드), `~/.bashrc` 유지

## 단축키

| 키 | 동작 |
|---|---|
| Ctrl+Shift+T / `+` | 새 탭 |
| Ctrl+Shift+W / 휠클릭 / × | 탭 닫기 |
| Ctrl+Tab / Ctrl+Shift+Tab | 다음 / 이전 탭 |
| Alt+1~9 | N번째 탭 (9 = 마지막) |
| Ctrl+Shift+C / Ctrl+V | 복사 / 붙여넣기 |
| 우클릭 | 선택 있으면 복사, 없으면 붙여넣기 |

## 실행 / 빌드

Git for Windows가 기본 경로(`C:\Program Files\Git`)에 설치되어 있어야 합니다.

```bash
npm install
npm start        # 개발 실행
npm run dist     # release/win-unpacked/Git Bash Tabs.exe 생성
```

`npm run dist`가 winCodeSign 압축 해제 중 심볼릭 링크 오류로 실패하면, `%LOCALAPPDATA%\electron-builder\Cache\winCodeSign\` 안의 `.7z`를 `darwin` 폴더를 제외하고 `winCodeSign-2.6.0` 폴더로 직접 풀어주면 됩니다.

## 아이콘

`icon.ico` / `icon.png`는 [Git for Windows](https://gitforwindows.org/) 아이콘입니다.

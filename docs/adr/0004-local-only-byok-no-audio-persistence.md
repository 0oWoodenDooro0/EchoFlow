---
status: accepted
---

# 純本機 BYOK 架構、OS 原生憑證儲存與純文字歷史

本應用定位為純本機、零後端、無帳號系統的 BYOK 工具。Provider Credential 預設存入作業系統原生憑證庫（macOS Keychain / Windows Credential Manager / Linux Secret Service），絕不以明文寫入設定檔；本機 SQLite 的 History Entry 只保存純文字與時間戳（預設保留 30 天自動清理），不留存任何原始音訊檔。

## Considered Options

- **自建後端同步帳號與歷史**：大幅增加維運、資安合規與伺服器成本，與 BYOK 隱私定位背離。
- **本機暫存原始音訊檔以便失敗重試**：增加硬碟佔用與隱私風險，且在純 Live 串流架構下重播 PCM 的邊際價值低。

## Consequences

- 跨裝置帶走設定與 Custom Vocabulary 改由本機 JSON 匯出／匯入完成（匯出內容明確排除 Provider Credential）。
- 若 Dictation Session 因網路中斷而異常結束，系統必須將已收到的部分文字寫入 History Entry 與剪貼簿，避免內容憑空遺失。

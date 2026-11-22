---
status: accepted
---

# 懸浮條即時預覽並於放開熱鍵時透過剪貼簿一次上字

雖然底層採即時串流轉錄，我們決定在 Dictation Session 進行期間僅於無焦點的 Overlay Pill 顯示即時 Draft Transcript，待使用者放開 Push-to-Talk Hotkey 取得 Final Transcript 後，再透過 Clipboard Preservation（備份原剪貼簿 → 寫入 Final Transcript → 模擬 `Cmd+V` / `Ctrl+V` 貼上 → 延遲還原原剪貼簿）一次完成 Text Insertion。

## Considered Options

- **邊講邊直接打字進目標 App（Live Typing）**：`SMART` 模式的句中自我修正會頻繁修改前文，必須送出大量退格鍵，在 Terminal、IDE 與中文輸入法開啟時極易錯亂。
- **作業系統逐字模擬鍵盤事件**：不碰剪貼簿，但在跨平台中文、中英夾雜、Emoji 與換行字元上極易掉字或受 IME 狀態干擾。

## Consequences

- 使用者在講話當下仍享有即時文字回饋，且目標 App 不會出現文字跳動或退格錯位。
- 必須提供選項允許使用者關閉「自動還原剪貼簿」，並在貼上失敗時自動轉入 Fallback Delivery（保留於剪貼簿並提示）。

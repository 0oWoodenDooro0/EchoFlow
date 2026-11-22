# Voice Dictation

跨平台本機語音聽寫系統，透過全域按住說話將即時語音串流轉換為乾淨的文字並輸入至任何作用中的應用程式。

## Language

### 聽寫生命週期

**Dictation Session**:
單次從按下並按住熱鍵開始，到放開熱鍵或達 10 分鐘上限結束的完整語音聽寫生命週期。
_Avoid_: Recording, Call, Stream, 錄音檔

**Push-to-Talk Hotkey**:
用來觸發與維持 Dictation Session 的全域按鍵或組合鍵；按下開始、放開結束。
_Avoid_: Shortcut, Trigger, Keybinding, 快捷鍵

**Draft Transcript**:
Dictation Session 進行期間持續接收並可能被模型回頭修正的即時暫時文字。
_Avoid_: Partial text, Interim result, Live caption, 即時字幕

**Final Transcript**:
Dictation Session 結束後已定稿、已完成去贅詞與自我修正且不再變動的最終文字。
_Avoid_: Output, Result, Polished text, 定稿結果

### 系統互動與交付

**Overlay Pill**:
Dictation Session 進行時顯示於螢幕上、不搶奪系統輸入焦點的懸浮狀態列，用來呈現即時音波與 Draft Transcript。
_Avoid_: Popup, Toast, HUD, Modal, Widget, 彈窗

**Text Insertion**:
將 Final Transcript 寫入使用者當前作用中應用程式游標位置的系統級操作。
_Avoid_: Typing, Pasting, Injection, Auto-type, 自動打字

**Clipboard Preservation**:
在 Text Insertion 期間暫時備份使用者原本的剪貼簿內容，並在寫入完成後自動還原的保護機制。
_Avoid_: Clipboard backup, Paste hack

**Fallback Delivery**:
當系統環境或焦點狀態不允許直接完成 Text Insertion 時，將 Final Transcript 保留於剪貼簿並提示使用者手動貼上的降級交付方式。
_Avoid_: Error mode, Clipboard-only mode, Degraded paste

### 使用者資料與憑證

**Custom Vocabulary**:
使用者自行維護的專有名詞、人名與術語清單（上限 1,000 筆），用於在聽寫時提高特定詞彙的辨識權重。
_Avoid_: Dictionary, Word list, Glossary, Prompt hints, 詞庫

**History Entry**:
已結束的 Dictation Session 留存在本機的純文字紀錄，包含 Final Transcript 與時間戳，不含原始音訊。
_Avoid_: Log, Recording history, Transcript archive, 錄音紀錄

**Provider Credential**:
使用者自行提供並安全保存於本機系統憑證庫的語音服務存取金鑰。
_Avoid_: Secret, Token, Account key, Config key

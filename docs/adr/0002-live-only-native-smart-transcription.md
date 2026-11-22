---
status: accepted
---

# 單一採用 Gemini 3.5 Transcribe Live 與原生 SMART 模式

為維持極低延遲的即時聽寫體驗並簡化 v1 架構，我們決定只串接 `gemini-3.5-transcribe-live`（WebSocket 16-bit PCM 雙向串流），並直接依賴其原生 `SMART` 模式完成去贅詞、句中自我修正、標點與格式整理；v1 不支援 `gemini-3.5-transcribe` 批次模式，也不在轉錄後串接第二段文字 LLM 進行後處理。

## Considered Options

- **Live + Batch 雙模式切換**：增加檔案上傳、批次狀態機與雙套設定的維護成本。
- **Live 轉錄後加一層二次 LLM 潤飾（如 Gemini Flash）**：可自訂語氣改寫，但會增加 0.5–1.5 秒尾端延遲與額外 token 費用。

## Consequences

- 必須處理 `gemini-3.5-transcribe-live` 單次連線 10 分鐘的硬上限：因本應用採純 Push-to-Talk，於 9:30 在 Overlay Pill 倒數提醒，滿 10:00 自動收尾並執行 Text Insertion。
- 自訂 Prompt 語氣改寫、翻譯模式與長會議錄音歸屬 v2 擴充。

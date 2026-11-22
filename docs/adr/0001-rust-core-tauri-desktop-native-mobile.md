---
status: accepted
---

# Rust 共用核心搭配 Tauri v2 桌面端與原生行動端殼層

本專案採分期推進（Phase 1 Desktop：macOS / Windows / Linux；Phase 2 Mobile：iOS / Android）。我們決定將音訊擷取與重採樣、即時串流客戶端、本機歷史與自訂詞典儲存全部實作於單一共用 Rust 核心，Desktop 端以 Tauri v2（React + TypeScript + Tailwind CSS）包裝，Phase 2 Mobile 端則以原生 Swift / Kotlin 自訂鍵盤殼層透過 UniFFI 呼叫同一個 Rust 核心。

## Considered Options

- **Electron + TypeScript（Desktop）搭配獨立 Mobile**：常駐桌面記憶體過高，且行動端仍須重寫底層音訊與串流邏輯。
- **Flutter 全平台**：桌面系統級熱鍵與無焦點懸浮窗生態較弱，且 iOS 自訂鍵盤擴充有嚴格的 ~50MB 記憶體上限，無法穩定承載 Flutter Engine。
- **五平台全原生各自開發**：系統整合最佳，但需維護五份重複的串流與領域邏輯。

## Consequences

- Desktop 主設定視窗關閉時應釋放 WebView，使平時常駐記憶體僅維持在 Rust 核心與原生系統匣層級。
- Rust 核心必須維持平台無關的邊界，將 OS 權限、熱鍵監聽、剪貼簿與上字等能力抽象為可由各平台殼層實作的介面。

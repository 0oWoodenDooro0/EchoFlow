---
status: accepted
---

# Linux 採 X11 完整支援與 Wayland 漸進降級

Linux Wayland 合成器出於安全隔離，未提供跨桌面統一的全域按鍵放開（key-up）監聽與合成貼上 API。為避免被 Wayland 碎片化卡住 Phase 1，我們決定在 Linux X11 提供完整的 Push-to-Talk Hotkey 與自動 Text Insertion；在 Wayland 環境則採漸進支援——偵測到可用權限或工具（如 `evdev`、`ydotool`、`wtype`）時啟用完整能力，否則自動降級為系統快捷鍵觸發搭配 Fallback Delivery。

## Considered Options

- **Phase 1 強制所有主流 Wayland 合成器零設定開箱即用**：需針對 GNOME、KDE、Hyprland/wlroots 分別維護不同協定與提權流程，工程成本與維護風險過高。
- **完全放棄 Wayland**：會排除多數現代 Linux 桌面使用者。

## Consequences

- 系統啟動與設定頁需能偵測目前 Linux 顯示伺服器型態與可用權限，並清楚告知使用者目前處於完整模式或 Fallback Delivery 模式。

import { jsx as _jsx, jsxs as _jsxs } from "react/jsx-runtime";
import { createContext, useContext } from "react";
import { createPortal } from "react-dom";
import { Button, IconButton, TextField } from "@sarmg/admin-ui";
import { DEFAULT_WORKSPACE_CONFIG, WORKSPACE_ICON_PATHS, validInstanceName } from "./workspace-config.js";
export const WorkspaceContext = createContext(DEFAULT_WORKSPACE_CONFIG);
export const HeaderActionsContext = createContext(null);
export const HeaderNavigationContext = createContext(null);
export function HeaderNavigation({ children, label = "页面导航" }) {
    const target = useContext(HeaderNavigationContext);
    return target ? createPortal(_jsx("nav", { className: "sarmg-header-navigation", "aria-label": label, children: children }), target) : null;
}
export function HeaderActions({ children }) {
    const target = useContext(HeaderActionsContext);
    return target ? createPortal(children, target) : null;
}
export function WorkspaceIcon({ name }) {
    return _jsx("svg", { "aria-hidden": "true", width: "22", height: "22", viewBox: "0 0 24 24", fill: "none", stroke: "currentColor", strokeWidth: "1.7", strokeLinecap: "round", strokeLinejoin: "round", children: _jsx("path", { d: WORKSPACE_ICON_PATHS[name] }) });
}
export function InstanceHeaderActions({ create, refresh, refreshing = false, createLabel = "新建实例", refreshLabel = "刷新" }) {
    const config = useContext(WorkspaceContext);
    return _jsxs(HeaderActions, { children: [create && _jsx(IconButton, { "aria-label": createLabel, title: createLabel, onClick: create, children: config.headerControls === "icons" ? _jsx(WorkspaceIcon, { name: "create" }) : createLabel }), refresh && _jsx(IconButton, { "aria-label": refreshLabel, title: refreshLabel, onClick: refresh, disabled: refreshing, children: config.headerControls === "icons" ? _jsx(WorkspaceIcon, { name: "refresh" }) : refreshLabel })] });
}
export function InstanceWorkspace({ instances, selected, select, label = "实例", showSidebar = true, children }) {
    const config = useContext(WorkspaceContext);
    const sidebarVisible = showSidebar && instances.length > 0;
    return _jsxs("div", { className: config.layout === "instances" ? `sarmg-instance-workspace${sidebarVisible ? "" : " sarmg-instance-workspace--full"}` : "sarmg-custom-workspace", children: [sidebarVisible && _jsx("aside", { className: "sarmg-instance-sidebar", "aria-label": label, children: _jsx("div", { className: "sarmg-instance-list", children: instances.map(item => _jsx(Button, { title: item.name, "aria-label": `选择实例 ${item.name}`, "aria-pressed": selected === item.id, onClick: () => select(item.id), children: _jsx("span", { children: item.name }) }, item.id)) }) }), _jsx("section", { "aria-label": "\u5B9E\u4F8B\u8BE6\u60C5\u4E0E\u8BBE\u7F6E", children: children })] });
}
/** Count Unicode scalar values, matching Rust chars(); do not use UTF-16 maxLength. */
export function InstanceNameField({ onChange, onInput, ...props }) {
    const config = useContext(WorkspaceContext);
    const validate = (input) => input.setCustomValidity(validInstanceName(input.value, config.instanceNameMaxCharacters) ? "" : `名称须为 1–${config.instanceNameMaxCharacters} 个字符，不能包含控制字符`);
    return _jsx(TextField, { ...props, required: true, pattern: `[^\\x00-\\x1f\\x7f]{1,${config.instanceNameMaxCharacters}}`, onChange: event => { validate(event.currentTarget); onChange?.(event); }, onInput: event => { validate(event.currentTarget); onInput?.(event); } });
}

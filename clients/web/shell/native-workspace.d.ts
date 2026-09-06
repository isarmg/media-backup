import { type WorkspaceConfig } from "./workspace-config.js";
/** Native Web adapter: retains existing product listeners, including logout/CSRF. */
export declare function configureNativeWorkspace({ header, content, actions, create, logout, refresh, instanceName, instanceHref, config: input }: {
    header: HTMLElement;
    content: HTMLElement;
    actions: HTMLElement;
    create?: HTMLButtonElement;
    logout: HTMLButtonElement;
    refresh(): void;
    instanceName: string;
    instanceHref: string;
    config?: Partial<WorkspaceConfig>;
}): void;

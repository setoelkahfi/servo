export interface InitOpts {
    url: string;
    resourceDir: string,
    commandlineArgs: string,
}

export const loadURL: (url: string) => void;
// False lets HarmonyOS handle Back when the focused tab has no previous page.
export const goBack: () => boolean;
export const goForward: () => void;
export const registerURLcallback: (callback: (url: string) => void) => void;
export const registerTerminateCallback: (callback: () => void) => void;
export const registerPromptToastCallback: (callback: (msg: string) => void) => void;
export const focusWebview:(index: number, arkts_ids:number[]) => void;
export const deleteWebview:(index: number, arkts_ids:number[]) => void;
export const initServo:(options: InitOpts) => void;
export const nextWindowId:(arkts_id: number) => void;

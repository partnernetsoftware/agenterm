/* Owned, non-activating Win32 wheel fixture for cu-pointer-scroll-smoke.qjs. */
#define UNICODE
#define _UNICODE
#include <windows.h>
#include <stdio.h>
#include <wchar.h>

static wchar_t state_path[MAX_PATH];
static LONG sequence_number = 0;
static LONG content_offset = 500;

static void publish_state(void) {
    wchar_t temporary[MAX_PATH];
    _snwprintf_s(temporary, MAX_PATH, _TRUNCATE, L"%ls.tmp", state_path);
    FILE *file = NULL;
    if (_wfopen_s(&file, temporary, L"wb") != 0 || file == NULL) ExitProcess(21);
    fprintf(file, "{\"ready\":true,\"sequence\":%ld,\"content_offset\":%ld}\n",
            sequence_number, content_offset);
    if (fclose(file) != 0) ExitProcess(22);
    if (!MoveFileExW(temporary, state_path, MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH)) {
        ExitProcess(23);
    }
}

static LRESULT CALLBACK window_proc(HWND window, UINT message, WPARAM wparam, LPARAM lparam) {
    (void)lparam;
    if (message == WM_MOUSEWHEEL || message == WM_MOUSEHWHEEL) {
        LONG delta = (LONG)(SHORT)HIWORD(wparam);
        content_offset += delta;
        sequence_number += 1;
        publish_state();
        return 0;
    }
    if (message == WM_DESTROY) { PostQuitMessage(0); return 0; }
    return DefWindowProcW(window, message, wparam, lparam);
}

int wmain(int argc, wchar_t **argv) {
    if (argc != 2 || wcslen(argv[1]) >= MAX_PATH) return 2;
    wcscpy_s(state_path, MAX_PATH, argv[1]);
    HINSTANCE instance = GetModuleHandleW(NULL);
    WNDCLASSW cls = {0};
    cls.lpfnWndProc = window_proc;
    cls.hInstance = instance;
    cls.lpszClassName = L"AgentermPointerScrollFixture";
    cls.hCursor = LoadCursorW(NULL, IDC_ARROW);
    if (!RegisterClassW(&cls)) return 3;
    POINT pointer;
    if (!GetCursorPos(&pointer)) return 4;
    HWND window = CreateWindowExW(WS_EX_NOACTIVATE | WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
        cls.lpszClassName, L"agenterm-pointer-scroll-fixture", WS_POPUP,
        pointer.x - 120, pointer.y - 90, 240, 180, NULL, NULL, instance, NULL);
    if (window == NULL) return 5;
    ShowWindow(window, SW_SHOWNOACTIVATE);
    UpdateWindow(window);
    publish_state();
    MSG message;
    while (GetMessageW(&message, NULL, 0, 0) > 0) {
        TranslateMessage(&message);
        DispatchMessageW(&message);
    }
    return 0;
}

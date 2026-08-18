// Pure UI state. Anything derived from the backend lives in TanStack Query.
import { create } from "zustand";

export type Theme = "dark" | "light";

interface UiState {
  theme: Theme;
  selectedInstanceId: number | null;
  setTheme: (theme: Theme) => void;
  selectInstance: (id: number | null) => void;
}

export function applyTheme(theme: Theme) {
  document.documentElement.classList.toggle("light", theme === "light");
}

export const useUiStore = create<UiState>((set) => ({
  theme: "dark",
  selectedInstanceId: null,
  setTheme: (theme) => {
    applyTheme(theme);
    set({ theme });
  },
  selectInstance: (selectedInstanceId) => set({ selectedInstanceId }),
}));

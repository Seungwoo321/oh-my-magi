import { useSyncExternalStore } from "react";
import translations from "../locales/en.json";
import messages from "../locales/messages.json";
export type Locale = "ko" | "en";
const english: Record<string, string> = translations;
let locale: Locale = "ko";
const listeners = new Set<() => void>();
export function setLocale(next: Locale): void {
  if (locale === next) return;
  locale = next;
  for (const listener of listeners) listener();
}
export function getLocale(): Locale { return locale; }
export function useLocale(): Locale {
  return useSyncExternalStore((listener) => { listeners.add(listener); return () => listeners.delete(listener); }, getLocale, () => "ko");
}
export function t(message: string, parameters: unknown[] = []): string {
  const translated = locale === "en" ? english[message] || message : message;
  return translated.replace(/\{(\d+)\}/g, (_match, index: string) => {
    const value = parameters[Number(index)];
    return typeof value === "string" && locale === "en" ? english[value] || value : String(value ?? "");
  });
}
export function hasEnglishMessages(messages: readonly string[]): boolean {
  return messages.length > 0 && messages.every((message) => Boolean(english[message]));
}
export const englishReady = hasEnglishMessages(messages);

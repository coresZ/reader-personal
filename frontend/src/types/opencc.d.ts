declare module '*chinese.js' {
  /** 简体 → 繁体。 */
  export function traditionalized(text: string): string;
  /** 繁体 → 简体。 */
  export function simplified(text: string): string;
}

/** Bil-läge: körläge på den här datorn. Ingen ny ljudmotor och ingen andra kö. */

export const DRIVE_HIDDEN_SURFACES = [
  "library",
  "settings",
  "import",
  "whisper",
  "statistics",
] as const;

export type DriveSurface = (typeof DRIVE_HIDDEN_SURFACES)[number];

export const DRIVE_SKIP_SECONDS = [-30, -15, 15, 30] as const;

export type DriveSkipSeconds = (typeof DRIVE_SKIP_SECONDS)[number];

export const DRIVE_LABELS = {
  enter: "Bil-läge",
  exit: "Lämna bil-läge",
  play: "Spela",
  pause: "Paus",
  back30: "−30",
  back15: "−15",
  forward15: "+15",
  forward30: "+30",
  previous: "Föregående",
  next: "Nästa",
  resume: "Fortsätt där du var",
  queue: "Kö",
  remove: "Ta bort",
  confirm: "Bekräfta",
  cancel: "Avbryt",
  confirmRemove: "Ta bort från kön?",
  emptyQueue: "Kön är tom.",
  nothingToResume: "Inget att fortsätta.",
} as const;

export type DriveChrome = {
  library: boolean;
  settings: boolean;
  import: boolean;
  whisper: boolean;
  statistics: boolean;
  seekSlider: boolean;
  drive: boolean;
  queue: boolean;
};

export function hiddenSurfaces(driveMode: boolean): readonly DriveSurface[] {
  return driveMode ? DRIVE_HIDDEN_SURFACES : [];
}

export function visibleChrome(driveMode: boolean): DriveChrome {
  const hidden = new Set<string>(hiddenSurfaces(driveMode));
  return {
    library: !hidden.has("library"),
    settings: !hidden.has("settings"),
    import: !hidden.has("import"),
    whisper: !hidden.has("whisper"),
    statistics: !hidden.has("statistics"),
    seekSlider: !driveMode,
    drive: driveMode,
    queue: true,
  };
}

/** Endast de fasta hoppen. Fri seek (reglage, godtycklig tid) är av i bil-läge. */
export function driveSkipSeconds(delta: number): DriveSkipSeconds | null {
  if (delta === -30 || delta === -15 || delta === 15 || delta === 30) return delta;
  return null;
}

/**
 * Föregående i bil-läge byter köpost. Den hoppar inte till början av avsnittet.
 * `index` är aktuell post, eller -1 om den inte ligger i kön.
 */
export function drivePreviousIndex(index: number): number | null {
  if (index > 0) return index - 1;
  return null;
}

export function driveNextIndex(index: number, length: number): number | null {
  if (length <= 0) return null;
  if (index >= 0 && index + 1 < length) return index + 1;
  if (index < 0 && length > 0) return 0;
  return null;
}

export type DestructiveKind = "remove-queue-item";

export function destructiveNeedsConfirmation(kind: DestructiveKind): true {
  void kind;
  return true;
}

export function commitDestructive(pendingId: number | null, queueId: number): boolean {
  return pendingId === queueId;
}

/**
 * Första steget armerar bara. Borttagning sker när samma kö-id bekräftas.
 * Ett annat id, eller bekräftelse utan arm, tar inte bort något.
 */
export function removalStep(
  pendingId: number | null,
  queueId: number,
  confirm: boolean,
): { pendingId: number | null; remove: boolean } {
  if (!destructiveNeedsConfirmation("remove-queue-item")) {
    return { pendingId: null, remove: true };
  }
  if (!confirm) {
    return { pendingId: queueId, remove: false };
  }
  if (!commitDestructive(pendingId, queueId)) {
    return { pendingId, remove: false };
  }
  return { pendingId: null, remove: true };
}

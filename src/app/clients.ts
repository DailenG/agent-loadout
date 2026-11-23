// The client Loadout recommends (#2) and the presentation order it gets in
// every list the app orders. Presentation only: the core adapter table, its
// detection and apply order stay unchanged.

/** The recommended client's id; change it here to recommend another client. */
export const RECOMMENDED_CLIENT = "omp";

/** True for the recommended client's id. */
export function isRecommended(clientId: string): boolean {
  return clientId === RECOMMENDED_CLIENT;
}

/**
 * `items` with the recommended client first; everything else keeps its
 * incoming order. Returns a new array; without the recommended client it is
 * a copy in the same order.
 */
export function orderByRecommended<T>(items: readonly T[], idOf: (item: T) => string): T[] {
  const index = items.findIndex((item) => isRecommended(idOf(item)));
  if (index <= 0) {
    return [...items];
  }
  return [items[index], ...items.slice(0, index), ...items.slice(index + 1)];
}

/**
 * A comparator that puts the recommended client before others it would tie
 * with; chain it after a list's own sort key (detection, count).
 */
export function recommendedFirst<T>(idOf: (item: T) => string): (left: T, right: T) => number {
  return (left, right) => Number(isRecommended(idOf(right))) - Number(isRecommended(idOf(left)));
}

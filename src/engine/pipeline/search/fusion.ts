import type { SearchRecallTrace } from "../../types.js";

const RRF_K = 60;
const FTS_RRF_WEIGHT = 1.1;
const VECTOR_RRF_WEIGHT = 1;
const DISCORDANT_ROUTE_RRF_WEIGHT = 0.07;
const ROUTE_RANK_AGREEMENT_FACTOR = 2.5;
const MAX_CORROBORATION_RATIO = 0.25;
const FULL_FUSION_TAIL_MULTIPLIER = 2;

/** Scores one candidate from its best route, with cross-route corroboration. */
export function candidateFusionScore(
  recall: readonly SearchRecallTrace[],
  limit: number,
): number {
  const bestRankByRoute = new Map<"fts" | "vector", number>();
  for (const trace of recall) {
    if (!trace.found || trace.rank === undefined) continue;
    const previous = bestRankByRoute.get(trace.path);
    if (previous === undefined || trace.rank < previous) {
      bestRankByRoute.set(trace.path, trace.rank);
    }
  }

  const routeScores = [...bestRankByRoute].map(([mode, rank]) => ({
    mode,
    rank,
    score:
      (mode === "fts" ? FTS_RRF_WEIGHT : VECTOR_RRF_WEIGHT) / (RRF_K + rank),
  }));
  routeScores.sort((left, right) => right.score - left.score);
  const primary = routeScores[0];
  if (!primary) return 0;

  const ranks = routeScores.map((route) => route.rank);
  const minimumRank = Math.min(...ranks);
  const maximumRank = Math.max(...ranks);
  const deepCutoff = limit * FULL_FUSION_TAIL_MULTIPLIER;
  const sameFusionBand = maximumRank <= limit || minimumRank > deepCutoff;
  const routesAgree =
    routeScores.length > 1 &&
    sameFusionBand &&
    maximumRank <= minimumRank * ROUTE_RANK_AGREEMENT_FACTOR;
  const corroboration = routeScores
    .slice(1)
    .reduce((score, route) => score + route.score, 0);
  const corroborationBonus = routesAgree
    ? Math.min(corroboration, primary.score * MAX_CORROBORATION_RATIO)
    : DISCORDANT_ROUTE_RRF_WEIGHT * corroboration;

  return primary.score + corroborationBonus;
}

/**
 * Drive Alignment Scorer — Score how well memories align with SOUL drives
 */

import { Drive, extractKeywords } from './mod_io';

/** Default importance multiplier for drive-aligned memories */
export const ALIGNMENT_BOOST = 1.5;

/**
 * Score how well a memory content aligns with a set of drives
 * Returns a score from 0.0 (no alignment) to 1.0 (strong alignment)
 */
export function scoreAlignment(content: string, drives: Drive[]): number {
  if (drives.length === 0) return 0.0;

  const contentLower = content.toLowerCase();
  const contentWords = contentLower.split(/\s+/);

  let totalScore = 0;
  let matchedDrives = 0;

  for (const drive of drives) {
    let driveMatches = 0;
    const keywords = drive.keywords.length > 0 ? drive.keywords : extractKeywords(drive);

    for (const keyword of keywords) {
      // Check for exact word match or substring match
      if (contentWords.some(w => w.includes(keyword))) {
        driveMatches++;
      }
    }

    if (driveMatches > 0) {
      matchedDrives++;
      // Score contribution: min(1.0, matches / 3) - need at least 3 matches for full score
      const driveScore = Math.min(1.0, driveMatches / 3);
      totalScore += driveScore;
    }
  }

  if (matchedDrives === 0) return 0.0;

  // Average score across matched drives, capped at 1.0
  return Math.min(1.0, totalScore / matchedDrives);
}

/**
 * Calculate the importance boost for a memory based on drive alignment
 * Returns a multiplier (1.0 = no boost, ALIGNMENT_BOOST for perfect alignment)
 */
export function calculateImportanceBoost(content: string, drives: Drive[]): number {
  const alignment = scoreAlignment(content, drives);

  if (alignment <= 0.0) return 1.0; // No boost

  // Linear interpolation between 1.0 and ALIGNMENT_BOOST based on alignment
  return 1.0 + (ALIGNMENT_BOOST - 1.0) * alignment;
}

/**
 * Check if content is strongly aligned with any drive
 * Returns true if alignment score is above 0.5
 */
export function isStronglyAligned(content: string, drives: Drive[]): boolean {
  return scoreAlignment(content, drives) > 0.5;
}

/**
 * Find which drives a piece of content aligns with
 * Returns a list of [drive_name, alignment_score] pairs for aligned drives
 */
export function findAlignedDrives(content: string, drives: Drive[]): Array<[string, number]> {
  const contentLower = content.toLowerCase();
  const contentWords = contentLower.split(/\s+/);

  const aligned: Array<[string, number]> = [];

  for (const drive of drives) {
    const keywords = drive.keywords.length > 0 ? drive.keywords : extractKeywords(drive);

    let matches = 0;
    for (const keyword of keywords) {
      if (contentWords.some(w => w.includes(keyword))) {
        matches++;
      }
    }

    if (matches > 0) {
      const score = Math.min(1.0, matches / 3);
      aligned.push([drive.name, score]);
    }
  }

  // Sort by score descending
  aligned.sort((a, b) => b[1] - a[1]);
  return aligned;
}

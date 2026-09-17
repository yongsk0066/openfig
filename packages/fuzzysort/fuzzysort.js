/*
 *
 *
 *
 * NOTE: we copied and edited a local version of fuzzysort that only contains functions we require
 *
 *
 *
 */

var isNode = typeof require !== "undefined" && typeof window === "undefined";

// Bounded memo of prepared strings. Upstream used two plain Maps that were only emptied by
// `cleanup()`, which nothing called: with fuzzy search on, every suggestion name ever shown
// stayed prepared (50k names ~ +52 MiB) until the window reloaded.
//
// Two generations (the same scheme as `@fig/shared/lru`, inlined because this package has no
// dependencies): new entries go to `young`; when it holds `maxSize / 2` it becomes `old` and
// the previous `old` is dropped; a hit in `old` moves back to `young`. At most `maxSize`
// entries, and a hit in `young` is a single `Map#get` on this per-keystroke path.
function createBoundedCache(maxSize) {
  var generationSize = Math.floor(maxSize / 2);
  var young = new Map();
  var old = new Map();
  function insert(key, value) {
    if (young.size >= generationSize) {
      old = young;
      young = new Map();
    }
    young.set(key, value);
  }
  return {
    get: function (key) {
      var value = young.get(key);
      if (value !== undefined) return value;
      value = old.get(key);
      if (value === undefined) return undefined;
      old.delete(key);
      insert(key, value);
      return value;
    },
    set: function (key, value) {
      if (young.has(key)) young.set(key, value);
      else {
        old.delete(key);
        insert(key, value);
      }
    },
    clear: function () {
      young.clear();
      old.clear();
    },
    get size() {
      return young.size + old.size;
    },
  };
}

// A 5,000-name suggestion list (a large directory) stays prepared across keystrokes.
var PREPARED_CACHE_SIZE = 10000;
// Queries are prefixes of what the user types; far fewer distinct ones are live at a time.
var PREPARED_SEARCH_CACHE_SIZE = 1000;
var preparedCache = createBoundedCache(PREPARED_CACHE_SIZE);
var preparedSearchCache = createBoundedCache(PREPARED_SEARCH_CACHE_SIZE);
var noResults = [];
noResults.total = 0;
var matchesSimple = [];
var matchesStrict = [];
function cleanup() {
  preparedCache.clear();
  preparedSearchCache.clear();
  matchesSimple = [];
  matchesStrict = [];
}
function cacheSizes() {
  return { prepared: preparedCache.size, preparedSearch: preparedSearchCache.size };
}
function isObj(x) {
  return typeof x === "object";
} // faster as a function

/**
 * WHAT: SublimeText-like Fuzzy Search
 * USAGE:
 * fuzzysort.single('fs', 'Fuzzy Search') // {score: -16}
 * fuzzysort.single('test', 'test') // {score: 0}
 * fuzzysort.single('doesnt exist', 'target') // null
 *
 * fuzzysort.go('mr', ['Monitor.cpp', 'MeshRenderer.cpp'])
 * // [{score: -18, target: "MeshRenderer.cpp"}, {score: -6009, target: "Monitor.cpp"}]
 *
 * fuzzysort.highlight(fuzzysort.single('fs', 'Fuzzy Search'), '<b>', '</b>')
 * // <b>F</b>uzzy <b>S</b>earch
 */
export const fuzzysort = {
  single: function (search, target) {
    if (!search) return null;
    if (!isObj(search)) search = fuzzysort.getPreparedSearch(search);

    if (!target) return null;
    if (!isObj(target)) target = fuzzysort.getPrepared(target);
    return fuzzysort.algorithm(search, target, search[0]);
  },

  highlight: function (result, hOpen, hClose) {
    if (result === null) return null;
    if (hOpen === undefined) hOpen = "<b>";
    if (hClose === undefined) hClose = "</b>";
    var highlighted = "";
    var matchesIndex = 0;
    var opened = false;
    var target = result.target;
    var targetLen = target.length;
    var matchesBest = result.indexes;
    for (var i = 0; i < targetLen; ++i) {
      var char = target[i];
      if (matchesBest[matchesIndex] === i) {
        ++matchesIndex;
        if (!opened) {
          opened = true;
          highlighted += hOpen;
        }

        if (matchesIndex === matchesBest.length) {
          highlighted += char + hClose + target.substr(i + 1);
          break;
        }
      } else {
        if (opened) {
          opened = false;
          highlighted += hClose;
        }
      }
      highlighted += char;
    }

    return highlighted;
  },

  prepare: function (target) {
    if (!target) return;
    return {
      target: target,
      _targetLowerCodes: fuzzysort.prepareLowerCodes(target),
      _nextBeginningIndexes: null,
      score: null,
      indexes: null,
      obj: null,
    }; // hidden
  },
  prepareSearch: function (search) {
    if (!search) return;
    return fuzzysort.prepareLowerCodes(search);
  },

  getPrepared: function (target) {
    if (target.length > 999) return fuzzysort.prepare(target); // don't cache huge targets
    var targetPrepared = preparedCache.get(target);
    if (targetPrepared !== undefined) return targetPrepared;
    targetPrepared = fuzzysort.prepare(target);
    preparedCache.set(target, targetPrepared);
    return targetPrepared;
  },
  getPreparedSearch: function (search) {
    if (search.length > 999) return fuzzysort.prepareSearch(search); // don't cache huge searches
    var searchPrepared = preparedSearchCache.get(search);
    if (searchPrepared !== undefined) return searchPrepared;
    searchPrepared = fuzzysort.prepareSearch(search);
    preparedSearchCache.set(search, searchPrepared);
    return searchPrepared;
  },

  algorithm: function (searchLowerCodes, prepared, searchLowerCode) {
    var targetLowerCodes = prepared._targetLowerCodes;
    var searchLen = searchLowerCodes.length;
    var targetLen = targetLowerCodes.length;
    var searchI = 0; // where we at
    var targetI = 0; // where you at
    var matchesSimpleLen = 0;

    // very basic fuzzy match; to remove non-matching targets ASAP!
    // walk through target. find sequential matches.
    // if all chars aren't found then exit
    for (;;) {
      var isMatch = searchLowerCode === targetLowerCodes[targetI];
      if (isMatch) {
        matchesSimple[matchesSimpleLen++] = targetI;
        ++searchI;
        if (searchI === searchLen) break;
        searchLowerCode = searchLowerCodes[searchI];
      }
      ++targetI;
      if (targetI >= targetLen) return null; // Failed to find searchI
    }

    var searchI = 0;
    var successStrict = false;
    var matchesStrictLen = 0;

    var nextBeginningIndexes = prepared._nextBeginningIndexes;
    if (nextBeginningIndexes === null)
      nextBeginningIndexes = prepared._nextBeginningIndexes =
        fuzzysort.prepareNextBeginningIndexes(prepared.target);
    var firstPossibleI = (targetI =
      matchesSimple[0] === 0 ? 0 : nextBeginningIndexes[matchesSimple[0] - 1]);

    // Our target string successfully matched all characters in sequence!
    // Let's try a more advanced and strict test to improve the score
    // only count it as a match if it's consecutive or a beginning character!
    if (targetI !== targetLen)
      for (;;) {
        if (targetI >= targetLen) {
          // We failed to find a good spot for this search char, go back to the previous search char and force it forward
          if (searchI <= 0) break; // We failed to push chars forward for a better match

          --searchI;
          var lastMatch = matchesStrict[--matchesStrictLen];
          targetI = nextBeginningIndexes[lastMatch];
        } else {
          var isMatch = searchLowerCodes[searchI] === targetLowerCodes[targetI];
          if (isMatch) {
            matchesStrict[matchesStrictLen++] = targetI;
            ++searchI;
            if (searchI === searchLen) {
              successStrict = true;
              break;
            }
            ++targetI;
          } else {
            targetI = nextBeginningIndexes[targetI];
          }
        }
      }

    {
      // tally up the score & keep track of matches for highlighting later
      if (successStrict) {
        var matchesBest = matchesStrict;
        var matchesBestLen = matchesStrictLen;
      } else {
        var matchesBest = matchesSimple;
        var matchesBestLen = matchesSimpleLen;
      }
      var score = 0;
      var lastTargetI = -1;
      for (var i = 0; i < searchLen; ++i) {
        var targetI = matchesBest[i];
        // score only goes down if they're not consecutive
        if (lastTargetI !== targetI - 1) score -= targetI;
        lastTargetI = targetI;
      }
      if (!successStrict) score *= 1000;
      score -= targetLen - searchLen;
      prepared.score = score;
      prepared.indexes = new Array(matchesBestLen);
      for (var i = matchesBestLen - 1; i >= 0; --i)
        prepared.indexes[i] = matchesBest[i];

      return prepared;
    }
  },

  prepareLowerCodes: function (str) {
    var strLen = str.length;
    var lowerCodes = []; // new Array(strLen)    sparse array is too slow
    var lower = str.toLowerCase();
    for (var i = 0; i < strLen; ++i) lowerCodes[i] = lower.charCodeAt(i);
    return lowerCodes;
  },
  prepareBeginningIndexes: function (target) {
    var targetLen = target.length;
    var beginningIndexes = [];
    var beginningIndexesLen = 0;
    var wasUpper = false;
    var wasAlphanum = false;
    for (var i = 0; i < targetLen; ++i) {
      var targetCode = target.charCodeAt(i);
      var isUpper = targetCode >= 65 && targetCode <= 90;
      var isAlphanum =
        isUpper ||
        (targetCode >= 97 && targetCode <= 122) ||
        (targetCode >= 48 && targetCode <= 57);
      var isBeginning = (isUpper && !wasUpper) || !wasAlphanum || !isAlphanum;
      wasUpper = isUpper;
      wasAlphanum = isAlphanum;
      if (isBeginning) beginningIndexes[beginningIndexesLen++] = i;
    }
    return beginningIndexes;
  },
  prepareNextBeginningIndexes: function (target) {
    var targetLen = target.length;
    var beginningIndexes = fuzzysort.prepareBeginningIndexes(target);
    var nextBeginningIndexes = []; // new Array(targetLen)     sparse array is too slow
    var lastIsBeginning = beginningIndexes[0];
    var lastIsBeginningI = 0;
    for (var i = 0; i < targetLen; ++i) {
      if (lastIsBeginning > i) {
        nextBeginningIndexes[i] = lastIsBeginning;
      } else {
        lastIsBeginning = beginningIndexes[++lastIsBeginningI];
        nextBeginningIndexes[i] =
          lastIsBeginning === undefined ? targetLen : lastIsBeginning;
      }
    }
    return nextBeginningIndexes;
  },

  cleanup: cleanup,
  cacheSizes: cacheSizes,
};

export default fuzzysort;

// Backtracking regular expressions run off the UI thread. The owner terminates
// this worker when the query changes, including expressions still executing.
self.onmessage = ({data}) => {
  const {id, source, flags, text, limit} = data;
  try {
    const re = new RegExp(source, flags.includes('g') ? flags : flags + 'g');
    const ranges = [];
    let match, matched = false;
    while ((match = re.exec(text))) {
      matched = true;
      if (!match[0]) {
        re.lastIndex += re.unicode && text.codePointAt(re.lastIndex) > 0xffff ? 2 : 1;
        continue;
      }
      ranges.push([match.index, match.index + match[0].length]);
      if (ranges.length >= limit) break;
    }
    self.postMessage({id, ranges, matched});
  } catch {
    self.postMessage({id, ranges: []});
  }
};

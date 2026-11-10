import {highlight, highlightSegments, shellCommandHighlight} from './syntax.js?v=__SESSIONDOCK_ASSET_VERSION__';

self.onmessage = ({data}) => {
  const {source, kind, language, path} = data;
  let result = null;
  try {
    result = kind === 'command' ? shellCommandHighlight(source)
      : kind === 'tool' ? highlightSegments(source, path)
        : highlight(source, language, path);
  } catch { /* Optional decoration never prevents reading the original text. */ }
  self.postMessage(result);
};

//
// Copyright (c) 2025-2026 rustmailer.com (https://rustmailer.com)
//
// This file is part of the Bichon Email Archiving Project
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Affero General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU Affero General Public License for more details.
//
// You should have received a copy of the GNU Affero General Public License
// along with this program.  If not, see <http://www.gnu.org/licenses/>.


import React, { useCallback, useRef, useState } from 'react';

import { withCsp } from '@/lib/mail-csp';

interface EmailIframeProps {
  emailHtml: string;
  height?: number;
  /** Keep remote images/fonts loadable. Only set this when the user explicitly
   *  opted in to showing remote content for this message. */
  allowRemote?: boolean;
}

const EmailIframe: React.FC<EmailIframeProps> = ({ emailHtml, height, allowRemote = false }) => {
  const iframeRef = useRef<HTMLIFrameElement>(null);
  const [iframeHeight, setIframeHeight] = useState<number>(height ?? 300);

  const onLoad = useCallback(() => {
    try {
      const doc = iframeRef.current?.contentWindow?.document;
      if (doc?.body) {
        const h = Math.max(
          doc.body.scrollHeight,
          doc.body.offsetHeight,
          doc.documentElement.scrollHeight,
          doc.documentElement.offsetHeight,
        );
        if (h > 0) setIframeHeight(h + 20);
      }
    } catch {
      // sandbox prevents access — keep default height
    }
  }, []);

  return (
    <iframe
      ref={iframeRef}
      srcDoc={withCsp(emailHtml, allowRemote)}
      sandbox="allow-same-origin"
      scrolling="no"
      className="w-full border-none"
      title="Email Content"
      onLoad={onLoad}
      style={{ height: iframeHeight }}
    />
  );
};
export default EmailIframe;

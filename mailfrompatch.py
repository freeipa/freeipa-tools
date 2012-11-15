# Authors:
#   Martin Kosek <mkosek@redhat.com>
#
# Copyright (C) 2012  Red Hat
# see file 'COPYING' for use and warranty information
#
# This program is free software; you can redistribute it and/or modify
# it under the terms of the GNU General Public License as published by
# the Free Software Foundation, either version 3 of the License, or
# (at your option) any later version.
#
# This program is distributed in the hope that it will be useful,
# but WITHOUT ANY WARRANTY; without even the implied warranty of
# MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
# GNU General Public License for more details.
#
# You should have received a copy of the GNU General Public License
# along with this program.  If not, see <http://www.gnu.org/licenses/>.

from urllib import quote
import webbrowser
import sys
import os.path
import re
import string
import subprocess

def mailto(recipients, subject, body, attachments, mua="thunderbird"):
    """recipients: string with comma-separated emails (no spaces!)"""
    if not isinstance(attachments, (list, tuple)):
        attachments = [attachments]

    attachments = ",".join(attachment for attachment in attachments)
    if mua == 'thunderbird':
        mailto = "to=%s,subject=%s,body=%s,attachment=%s" % \
            (recipients, quote(subject), quote(body), quote(attachments))
        cmd = ['thunderbird', '-compose', mailto]
        subprocess.Popen(cmd)
    else:
        webbrowser.open("mailto:%s?subject=%s&body=%s&attachment=%s" %
        (recipients, quote(subject), quote(body), quote(attachments)))

def patch_number(filename):
    m = re.match(r'\D+-(\d+)-\S+.patch', os.path.basename(filename))
    if m is None:
        return None
    return m.group(1)

def mail_from_patch(filenames, recipient):
    if not isinstance(filenames, (list, tuple)):
        filenames = [filenames]
    patches = []
    for filename in filenames:
        # parse patch to get SUBJECT and BODY
        subject=""
        body=""
        skip_a_line = False
        with open(filename, 'r') as f:
            for line in f:
                if skip_a_line:
                    skip_a_line = False
                    continue

                if not subject:
                    m = re.match(r'Subject: (.+\S)\s*$', line)
                    if m is not None:
                        subject = m.group(1)

                        m = re.match(r'(\[PATCH\])(.+)$', subject)
                        if m:
                            subject = m.group(2)

                        skip_a_line = True
                        continue
                else:
                    # do not include patch itself to file
                    if string.strip(line) == '---':
                        break
                    else:
                        body = body + line
        patches.append({'filename': filename,
                        'filename_basename': os.path.basename(filename),
                        'subject': subject.strip(),
                        'body': body})

    # simply take the last subject
    if len(patches) > 1:
        subject = '[PATCH] %s-%s %s' % (patch_number(patches[0]['filename']),
                                        patch_number(patches[-1]['filename']),
                                        patches[-1]['subject'])
        patch_format = "[%(filename_basename)s]:\n\n%(body)s"
    else:
        subject = '[PATCH] %s %s' % (patch_number(patches[0]['filename']),
                                     patches[0]['subject'])
        patch_format = "%(body)s"
    body = u"\n".join(patch_format % patch for patch in patches)
    filenames = tuple(patch['filename'] for patch in patches)

    # create mail
    mailto(recipient, subject, body, filenames)

########################### PROGRAM WAS EXECUTED ###########################
if __name__ == "__main__":
    if len(sys.argv) != 2:
        sys.exit("Usage: " + sys.argv[0] + " PATH_TO_PATCH")

    if not os.path.exists(sys.argv[1]):
        sys.exit("Target patch not found!")

    mail_from_patch(sys.argv[1], 'freeipa-devel@redhat.com')

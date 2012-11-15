#!/usr/bin/python
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

import subprocess
import re
import os
import string
import sys
from optparse import OptionParser
from mailfrompatch import mail_from_patch
from patchtotrac import patch_to_trac, get_tickets_from_patch

PATCH_DIR = os.path.join(os.path.expanduser('~'), 'patches')
PATCH_PREFIX = 'freeipa-mkosek'

target_patches = []

def main():
    # parse arguments
    usage = "usage: %prog [options] PATCH_COUNT"
    parser = OptionParser(usage=usage)

    (options, args) = parser.parse_args()
    try:
        patch_count = args[0]
    except IndexError:
        patch_count = 1
    else:
        try:
            patch_count = int(args[0])
        except ValueError:
            sys.exit('Wrong PATCH_COUNT number')
        else:
            if patch_count < 1:
                sys.exit("PATCH_COUNT must be greater than 1")

    # Get the greatest patch number
    if not os.path.exists(PATCH_DIR) or not os.path.isdir(PATCH_DIR):
        print PATCH_DIR, "does not exists or is not a directory!"
        exit(1)

    patches = os.listdir(PATCH_DIR)
    max_patch_num = 0

    for patch in patches:
        m = re.match(r'%s-(\d+)(\.\d+)?-(\S+).patch' % PATCH_PREFIX, patch)

        if m is None:   # did not match, pass
            print "Unknown patch format:", patch
            continue

        patch_num = int(m.group(1))
        if max_patch_num < patch_num:
            max_patch_num = patch_num

    patch_num = max_patch_num + 1

    # Create GIT patch
    cmd = [ 'git', 'format-patch', '-M', '-C', '--patience', '--full-index', '-%d' % patch_count ]
    output = subprocess.Popen( cmd, stdout=subprocess.PIPE ).communicate()[0]
    git_patches = [ line for line in output.split("\n") if line ]

    global target_patches
    for git_patch in git_patches:
        print "Processing patch %s" % git_patch
        patch_match = re.match(r'(\d+)-(\S+).patch', git_patch)

        if patch_match is None:
            print "GIT format-patch produced unknown output:", output
            exit(2)

        source_patch_path = patch_match.group(0)

        # Get user text
        patch_desc = string.lower(patch_match.group(2))
        user_desc_text = raw_input("Patch description [" + patch_desc + "]: ")

        m = re.match(r'\s*([a-z0-9-]*)\s*$', user_desc_text)

        if m is None:
            print "Invalid user file description:", m.group(1)
            exit (3)

        if m.group(1):
            patch_desc = m.group(1)

        target_patch_name = "%s-%03d-%s.patch" % (PATCH_PREFIX, patch_num, patch_desc)
        patch_num += 1

        # check diff
        patch_ok=True
        line_no = 1
        with open(source_patch_path, 'r') as f:
            for line in f:
                # check for tabs
                if line.find('\t') != -1:
                    print "Line %d | [TAB]:" % line_no, line,
                    patch_ok = False

                m = re.match(r'(\+)(.*)( +)$', line)
                if m is not None:
                    print "Line %d | [WHITESPACE]:"  % line_no, line,
                    patch_ok = False

                line_no += 1

        approval = None
        if not patch_ok:
            while approval != 'y' and approval != 'n' and approval != '':
                approval = raw_input("Patch precheck failed! Continue [y]: ").lower()

        if approval == 'n':
            print "Abort operation"
            os.remove(source_patch_path)
            exit(5)

        # move patch to patch folder
        target_patch_path = os.path.join(PATCH_DIR, target_patch_name)

        os.rename(source_patch_path, target_patch_path)

        target_patches.append(target_patch_path)

    # now we have all patches moved to right location
    tickets = {}
    for target_patch in target_patches:
        for ticket in get_tickets_from_patch(target_patch):
            tickets.setdefault(ticket, []).append(target_patch)

    mail_from_patch(target_patches, 'freeipa-devel@redhat.com')

    if not tickets:
        print "No ticket ID in patch %s detected!" % target_patch_path
    else:
        for ticket, patches in tickets.iteritems():
            print "Ticket #", ticket
            for patch in patches:
                print "  -", patch
            approval = None
            while approval != 'y' and approval != 'n' and approval != '':
                approval = raw_input("Update ticket %d [y]: " % ticket)

            if approval == 'n':
                continue

            patch_to_trac(ticket, target_patch_path)

if __name__ == "__main__":
    try:
        sys.exit(main())
    except KeyboardInterrupt:
        print >> sys.stderr, "\nCleaning up"

        if target_patches:
            for patch in target_patches:
                try:
                    print >> sys.stderr, "  - removing", patch
                    os.unlink(patch)
                except OSError, e:
                    print >> sys.stderr, "Cannot remove %s: %s" % (target_patch_path, str(e))
            sys.exit(1)

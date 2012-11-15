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

import xmlrpclib
import sys
import re
from os.path import abspath, basename
from base64 import b64decode
from optparse import OptionParser

USER = "mkosek"
PASSWORD = "password"
TRAC_URL = 'https://%s:%s@fedorahosted.org/freeipa/login/xmlrpc' % (USER, PASSWORD)
TRAC_TICKET_PATH = "https://fedorahosted.org/freeipa/ticket/"

_trac = xmlrpclib.ServerProxy(TRAC_URL)

def get_tickets_from_patch(patch):
    tickets = []
    with open(patch, 'r') as f:
        for line in f:
            if line.startswith(u'---'): # the actual patch starts
                break

            m = re.match(r'(\S+\s)?%s(\d+)' % TRAC_TICKET_PATH, line)
            if m is not None:
                ticket = m.group(2)
                tickets.append(int(ticket))

            m = re.match(r'ticket\s+#?(\d+)\s+$', line)
            if m is not None:
                ticket = m.group(1)
                tickets.append(int(ticket))

            m = re.match(r'Subject:.*ticket\s+#?(\d+)\s+', line)
            if m is not None:
                ticket = m.group(1)
                tickets.append(int(ticket))

    return tickets

def patch_to_trac(ticket, patch):
    # upload patch
    global _trac
    with open(patch, 'r+') as f:
        data = xmlrpclib.Binary(f.read())
        _trac.ticket.putAttachment(ticket, basename(patch), '', data, True)

    # change ticket status to assigned
    update = {'on_review' : '1', 'status' : 'assigned' }

    _trac.ticket.update(ticket, 'Patch \'\'%s\'\' sent for review' % basename(patch), update)

def main():
    # parse arguments
    usage = "usage: %prog [options] PATCH"
    parser = OptionParser(usage=usage)

    (options, args) = parser.parse_args()

    if not len(args):
            parser.error("No patch passed!")

    patch = abspath(args[0])

    tickets = get_tickets_from_patch(patch)

    if not tickets:
        print "No ticket ID in patch %s detected!" % patch
        sys.exit(1)

    for ticket in tickets:
        approval = None
        while approval != 'y' and approval != 'n' and approval != '':
            approval = raw_input("Update ticket %d [y]: " % ticket)

        if approval == 'n':
            continue

        patch_to_trac(ticket, patch)

if __name__ == "__main__":
    try:
        sys.exit(main())
    except KeyboardInterrupt:
        sys.exit("\nProgram interrupted")


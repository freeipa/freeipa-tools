#!/usr/bin/python
# -*- coding: utf-8 -*-

#
# Copyright (C) 2017  Martin Basti <mbasti@redhat.com>
#
# This program is free software: you can redistribute it and/or modify
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

from __future__ import print_function
import time
from libpagure import Pagure

PAGURE_REPO = "freeipa"
PAGURE_TOKEN = None; assert PAGURE_TOKEN, "Specify pagure token here"
MOVE_FROM = "FreeIPA 4.5.1"
MOVE_TO = "FreeIPA 4.5.2"
MESSAGE = "{} has been released, moving to {} milestone".format(
    MOVE_FROM, MOVE_TO)


def opened_tickets(pagure, milestone):
    tickets = pagure.list_issues(status="Open", milestones=[milestone])
    tickets_id = [(t['id'], t['title']) for t in tickets]
    return tickets_id


def move_tickets_to_milestone(pagure, move_from, move_to, message=None):
    """
    Move all opened tickets from one milestone to another
    :param pagure: Pagure instance
    :param move_from: source milestone
    :param move_to: destination milestone
    :param message: message to be added as comment to migrated issues
    """
    for t_num, t_title in opened_tickets(pagure, move_from):
        print("Moving ticket '#{}: {}' to milestone '{}'".format(
            t_num, t_title, move_to
        ))
        try:
            pagure.change_issue_milestone(t_num, move_to)
        except AttributeError:
            # Workaround https://pagure.io/libpagure/issue/23
            issue = pagure.issue_info(t_num)
            if issue['milestone'] != move_to:
                raise RuntimeError("Failed to move ticket to milestone")
        if message:
            pagure.comment_issue(t_num, message)
        time.sleep(5)  # Do not hammer pagure API

if __name__ == '__main__':
    pagure = Pagure(
        pagure_repository=PAGURE_REPO,
        pagure_token=PAGURE_TOKEN
    )
    move_tickets_to_milestone(pagure, MOVE_FROM, MOVE_TO, message=MESSAGE)
